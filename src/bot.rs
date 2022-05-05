use futures::future::LocalBoxFuture;
use log::{debug, info};
use serde::Serialize;
use serde_json::json;
use telegram_types::bot::methods::{
    ApiError, ChatTarget, DeleteWebhook, GetChat, GetChatMember, GetMe, Method, SetWebhook,
    TelegramResult, UpdateTypes,
};
use telegram_types::bot::types::{
    Chat, ChatMember, ChatMemberStatus, Message, Update, UpdateContent, User, UserId,
};
use worker::kv::KvStore;
use worker::wasm_bindgen::JsValue;
use worker::{
    Env, Error as WorkerError, Fetch, Headers, Method as RequestMethod, Request, RequestInit,
    Response, RouteContext,
};

use std::borrow::Cow;
use std::collections::HashMap;
use std::future::Future;
use std::rc::Rc;

const ACCEPTED_TYPES: &[UpdateTypes] = &[UpdateTypes::Message];

type CommandFn<'a> =
    Rc<dyn 'a + Fn(Message, Env, Bot<'a>) -> LocalBoxFuture<'a, Result<Response, WorkerError>>>;

#[derive(Clone)]
pub struct Bot<'a> {
    token: String,
    username: String,
    kv_store: String,
    commands: HashMap<String, CommandFn<'a>>,
}

#[derive(Clone, Debug, Serialize)]
pub struct WebhookReply<T: Method> {
    pub method: String,
    #[serde(flatten)]
    pub content: T,
}

impl<'a> Bot<'a> {
    pub fn new<S: AsRef<str>>(token: S, username: S, kv_store: S) -> Self {
        Self {
            token: token.as_ref().to_string(),
            username: username.as_ref().to_string(),
            kv_store: kv_store.as_ref().to_string(),
            commands: HashMap::new(),
        }
    }

    pub async fn send_json_request<T: Method>(
        &self,
        request: T,
        method: RequestMethod,
    ) -> Result<Response, WorkerError> {
        let mut request_builder = RequestInit::new();
        let mut headers = Headers::new();
        if method != RequestMethod::Get {
            headers.set("Content-Type", "application/json")?;
            let payload = serde_json::to_string(&request).map_err(Into::<WorkerError>::into)?;
            info!("Sending JSON payload: {}", payload);
            request_builder.with_body(Some(JsValue::from_str(&payload)));
        }
        request_builder.with_headers(headers).with_method(method);
        Fetch::Request(Request::new_with_init(
            &T::url(&self.token),
            &request_builder,
        )?)
        .send()
        .await
    }

    pub fn convert_error(e: ApiError) -> WorkerError {
        WorkerError::RustError(e.description)
    }

    pub async fn send_json_get<T: Method>(&self, request: T) -> Result<Response, WorkerError> {
        self.send_json_request(request, RequestMethod::Get).await
    }

    pub async fn get_me(&self) -> Result<User, WorkerError> {
        let mut result = self.send_json_get(GetMe).await?;
        result
            .json::<TelegramResult<User>>()
            .await?
            .into_result()
            .map_err(Bot::convert_error)
    }

    pub async fn get_chat(&self, chat_id: ChatTarget<'_>) -> Result<Chat, WorkerError> {
        let mut result = self
            .send_json_request(GetChat { chat_id }, RequestMethod::Post)
            .await?;
        result
            .json::<TelegramResult<Chat>>()
            .await?
            .into_result()
            .map_err(Bot::convert_error)
    }

    pub async fn is_admin(
        &self,
        chat_id: ChatTarget<'_>,
        user_id: UserId,
    ) -> Result<bool, WorkerError> {
        let chat_member = self
            .send_json_request(GetChatMember { chat_id, user_id }, RequestMethod::Post)
            .await?
            .json::<TelegramResult<ChatMember>>()
            .await?
            .into_result()
            .map_err(Bot::convert_error)?;
        let member_status = chat_member.status;
        info!("Member status: {:?}", member_status);
        Ok(member_status == ChatMemberStatus::Creator
            || member_status == ChatMemberStatus::Administrator)
    }

    // fn get_kv(&self) -> Result<KvStore, WorkerError> {
    //     self.env.kv(&self.env.var(VAR_KV_STORE)?.to_string())
    // }

    // pub async fn update_username(&mut self) -> Result<(), WorkerError> {
    //     let user = self.get_me().await?;
    //     self.username = user.username;
    //     let kv = self.get_kv()?;
    //     kv.put(KEY_USERNAME, self.username.clone().expect("WTF"))?
    //         .execute()
    //         .await?;
    //     Ok(())
    // }

    pub fn get_username(&self) -> String {
        self.username.clone()
    }

    pub fn new_with_env<S: AsRef<str>>(
        env: &Env,
        var_token: S,
        var_username: S,
        var_kv_store: S,
    ) -> Result<Self, WorkerError> {
        Ok(Self::new(
            env.secret(var_token.as_ref())?.to_string(),
            env.var(var_username.as_ref())?.to_string(),
            env.var(var_kv_store.as_ref())?.to_string(),
        ))
    }

    pub async fn setup_webhook<S: AsRef<str>>(&self, url: S) -> Result<(), WorkerError> {
        let user = self.get_me().await?;
        if user.username.expect("WTF, a bot without username???") != self.username {
            return Err(WorkerError::RustError("Username mismatched".to_string()));
        }
        let payload = DeleteWebhook;
        let mut result = self.send_json_request(payload, RequestMethod::Post).await?;
        info!(
            "Trying to delete previously set webhooks: {}",
            result.text().await?
        );
        let mut payload = SetWebhook::new(url.as_ref());
        payload.allowed_updates = Some(Cow::from(ACCEPTED_TYPES));
        let mut result = self.send_json_request(payload, RequestMethod::Post).await?;
        info!("Set new webhook: {}", result.text().await?);
        Ok(())
    }

    pub fn register_command<
        S: AsRef<str>,
        F: 'a + Future<Output = Result<Response, WorkerError>>,
    >(
        &mut self,
        command: S,
        func: fn(Message, Env, Bot<'a>) -> F,
    ) {
        self.commands.insert(
            command.as_ref().to_string(),
            Rc::new(move |msg, env, bot| Box::pin(func(msg, env, bot))),
        );
    }

    pub async fn run_commands(&self, m: Message, env: Env) -> Result<Response, WorkerError> {
        let message_text = m.text.clone().unwrap_or_default();
        info!("Non empty message text: {}", message_text);
        let message_command = message_text.split(' ').collect::<Vec<&str>>()[0]
            .trim()
            .to_ascii_lowercase();
        debug!("First phrase extracted from text: {}", message_command);
        for (command, func) in &self.commands {
            // `/start bruh` and `/start@blablabot bruh`
            let command_prefix = format!("/{}", command.to_ascii_lowercase());
            let command_prefix_extended = format!(
                "{}@{}",
                command_prefix,
                self.get_username().to_ascii_lowercase()
            );
            if (message_command == command_prefix) || (message_command == command_prefix_extended) {
                info!("Command matched: {}", command);
                return func(m, env, self.clone()).await;
            }
        }
        info!("No command matched, ignoring...");
        Response::empty()
    }

    pub async fn process_update(
        req: &mut Request,
        ctx: RouteContext<Bot<'a>>,
    ) -> Result<Response, WorkerError> {
        let update = req.json::<Update>().await?;
        debug!("Received update: {:?}", update);
        if update.content.is_none() {
            debug!("No content found, ignoring...");
            return Response::from_json(&json!({}));
        }
        let update_content = update.content.unwrap();
        if let UpdateContent::Message(m) = update_content {
            debug!("Got message: {:#?}", m);
            if m.text.is_none() {
                debug!("No text found, ignoring...");
                return Response::from_json(&json!({}));
            }
            let bot = ctx.data;
            let env = ctx.env;
            bot.run_commands(m, env).await
        } else {
            info!("Not a message, ignoring...");
            Response::from_json(&json!({}))
        }
    }

    pub fn get_kv(&self, env: &Env) -> Result<KvStore, WorkerError> {
        env.kv(&self.kv_store)
    }
}

impl<T: Method> From<T> for WebhookReply<T> {
    fn from(method: T) -> WebhookReply<T> {
        WebhookReply {
            method: <T>::NAME.to_string(),
            content: method,
        }
    }
}
