pub mod bot;
pub mod group;

use cfg_if::cfg_if;
use chrono_tz::Tz;
use log::{error, info};
use telegram_types::bot::methods::{ChatTarget, SendMessage};
use telegram_types::bot::types::{ChatId, Message};
use worker::{
    event, Date, Env, Error as WorkerError, Request, Response, Router, ScheduleContext,
    ScheduledEvent,
};

use bot::{Bot, WebhookReply};
use group::{get_group_title, get_raw_chat_id, DataStore, Group};

use std::num::ParseIntError;

const DEFAULT_SECRET_TOKEN: &str = "API_TOKEN";
const VAR_KV_STORE: &str = "KV_STORE";
const VAR_USERNAME: &str = "USERNAME";
// const DEFAULT_CRON_PATH: &str = "/cron";

cfg_if! {
    // https://github.com/rustwasm/console_error_panic_hook#readme
    if #[cfg(feature = "console_error_panic_hook")] {
        pub use console_error_panic_hook::set_once as set_panic_hook;
    } else {
        #[inline]
        pub fn set_panic_hook() {}
    }
}

pub fn return_message<S: AsRef<str>>(message: &Message, reply: S) -> Result<Response, WorkerError> {
    Response::from_json(&WebhookReply::from(
        SendMessage::new(ChatTarget::Id(message.chat.id), reply.as_ref()).reply(message.message_id),
    ))
}

pub fn warn_group_only(message: &Message) -> Result<Response, WorkerError> {
    return_message(message, "This command is only allowed in group chats")
}

pub async fn check_permission(
    group: &Group,
    m: &Message,
    bot: &Bot<'_>,
) -> Result<bool, WorkerError> {
    if group.require_admin {
        let user_id = m
            .from
            .clone()
            .ok_or_else(|| {
                WorkerError::RustError("Unable to retrieve user information".to_string())
            })?
            .id;
        bot.is_admin(ChatTarget::Id(m.chat.id), user_id).await
    } else {
        Ok(true)
    }
}

fn log_request(req: &Request) {
    info!(
        "{} - [{}], located at: {:?}, within: {}",
        Date::now().to_string(),
        req.path(),
        req.cf().coordinates().unwrap_or_default(),
        req.cf().region().unwrap_or_else(|| "unknown region".into())
    );
}

async fn update_template(
    store: &DataStore<'_>,
    group: &mut Group,
    bot: &Bot<'_>,
    m: &Message,
) -> Result<Response, WorkerError> {
    if group.enable
        && !group
            .apply_template(bot, &Date::now())
            .await
            .unwrap_or(false)
    {
        group.enable = false;
        store.save_group(group).await?;
        return return_message(m, "发生什么事了？未能成功更改群标题，请检查 bot 帐号权限");
    }
    store.save_group(group).await?;
    let reply = format!("标题模板已被更改至： {}", group.join_title_template());
    info!("Replied: {:?}", reply);
    return_message(m, reply)
}

pub async fn echo(m: Message, _env: Env, _bot: Bot<'_>) -> Result<Response, WorkerError> {
    let text = if let Some(msg) = m.text.clone().unwrap().split_once(' ') {
        msg.1.to_string()
    } else {
        "wut?".to_string()
    };
    return_message(&m, text)
}

pub async fn start(m: Message, _env: Env, _bot: Bot<'_>) -> Result<Response, WorkerError> {
    let reply = format!("Title bot {}", env!("CARGO_PKG_VERSION"));
    info!("Replied: {:?}", reply);
    return_message(&m, reply)
}

pub async fn status(m: Message, env: Env, bot: Bot<'_>) -> Result<Response, WorkerError> {
    let group_title = get_group_title(&m.chat.kind);
    if group_title.is_none() {
        return warn_group_only(&m);
    }
    let group_title = group_title.unwrap();

    let kv = bot.get_kv(&env)?;
    let store = DataStore::new(&kv);
    let group = store.load_group_or_create(&m.chat.id, &m.chat.kind).await;

    if !check_permission(&group, &m, &bot).await? {
        info!("Permission denied");
        return Response::empty();
    }

    let reply = format!(
        r#"当前标题: {}
           群 ID: {}
           启用自动更改: {}
           标题片段: {:?}
           分隔符: {}
           时区: {}
           需要管理权限: {}"#,
        group_title,
        get_raw_chat_id(&group.id),
        group.enable,
        group.title_segment,
        group.delimiter,
        group.timezone,
        group.require_admin
    );
    info!("Replied: {:?}", reply);
    return_message(&m, reply)
}

pub async fn enable(m: Message, env: Env, bot: Bot<'_>) -> Result<Response, WorkerError> {
    let group_title = get_group_title(&m.chat.kind);
    if group_title.is_none() {
        return warn_group_only(&m);
    }
    let kv = bot.get_kv(&env)?;
    let store = DataStore::new(&kv);
    let mut group = store.load_group_or_create(&m.chat.id, &m.chat.kind).await;

    if !check_permission(&group, &m, &bot).await? {
        return Response::empty();
    }

    group.enable = true;
    if !group
        .apply_template(&bot, &Date::now())
        .await
        .unwrap_or(false)
    {
        group.enable = false;
        store.save_group(&group).await?;
        return return_message(&m, "发生什么事了？未能成功更改群标题，请检查 bot 帐号权限");
    }
    store.save_group(&group).await?;
    let reply = format!(
        "已启用自动标题更改，当前标题模板为： {}",
        group.join_title_template()
    );
    info!("Enabled for group {}", get_raw_chat_id(&group.id));
    return_message(&m, reply)
}

pub async fn disable(m: Message, env: Env, bot: Bot<'_>) -> Result<Response, WorkerError> {
    let group_title = get_group_title(&m.chat.kind);
    if group_title.is_none() {
        return warn_group_only(&m);
    }
    let kv = bot.get_kv(&env)?;
    let store = DataStore::new(&kv);
    let mut group = store.load_group_or_create(&m.chat.id, &m.chat.kind).await;

    if !check_permission(&group, &m, &bot).await? {
        return Response::empty();
    }

    group.enable = false;
    store.save_group(&group).await?;
    info!("Disabled for group {}", get_raw_chat_id(&group.id));
    return_message(&m, "已禁用自动标题更改")
}

pub async fn set_template(m: Message, env: Env, bot: Bot<'_>) -> Result<Response, WorkerError> {
    let group_title = get_group_title(&m.chat.kind);
    if group_title.is_none() {
        return warn_group_only(&m);
    }
    let command = m.text.clone().unwrap();
    let title_template = command.split_once(' ');
    if title_template.is_none() {
        return return_message(&m, "无效命令，没有发现新的标题模板");
    }
    let kv = bot.get_kv(&env)?;
    let store = DataStore::new(&kv);
    let mut group = store.load_group_or_create(&m.chat.id, &m.chat.kind).await;

    if !check_permission(&group, &m, &bot).await? {
        return Response::empty();
    }

    group.clear_title_template();
    group.push_title_template(title_template.unwrap().1);
    update_template(&store, &mut group, &bot, &m).await
}

pub async fn set_delimiter(m: Message, env: Env, bot: Bot<'_>) -> Result<Response, WorkerError> {
    let group_title = get_group_title(&m.chat.kind);
    if group_title.is_none() {
        return warn_group_only(&m);
    }
    let command = m.text.clone().unwrap();
    let delimiter = command.split_once(' ');
    if delimiter.is_none() {
        return return_message(&m, "无效命令，没有发现新的分隔符");
    }
    let delimiter = delimiter.unwrap().1.to_string();
    let kv = bot.get_kv(&env)?;
    let store = DataStore::new(&kv);
    let mut group = store.load_group_or_create(&m.chat.id, &m.chat.kind).await;

    if !check_permission(&group, &m, &bot).await? {
        return Response::empty();
    }

    group.delimiter = delimiter;
    store.save_group(&group).await?;
    update_template(&store, &mut group, &bot, &m).await
}

pub async fn set_timezone(m: Message, env: Env, bot: Bot<'_>) -> Result<Response, WorkerError> {
    let group_title = get_group_title(&m.chat.kind);
    if group_title.is_none() {
        return warn_group_only(&m);
    }
    let command = m.text.clone().unwrap();
    let timezone_str = command.split_once(' ');
    if timezone_str.is_none() {
        return return_message(&m, "无效命令，没有发现新的时区名称");
    }
    let timezone_str = timezone_str.unwrap().1.to_string();
    let timezone: Result<Tz, _> = timezone_str.parse();
    if timezone.is_err() {
        return return_message(&m, "无效命令，无法解析时区名称");
    }
    let kv = bot.get_kv(&env)?;
    let store = DataStore::new(&kv);
    let mut group = store.load_group_or_create(&m.chat.id, &m.chat.kind).await;

    if !check_permission(&group, &m, &bot).await? {
        return Response::empty();
    }

    group.timezone = timezone.unwrap().to_string();
    if group.enable
        && !group
            .apply_template(&bot, &Date::now())
            .await
            .unwrap_or(false)
    {
        group.enable = false;
        store.save_group(&group).await?;
        return return_message(&m, "发生什么事了？未能成功更改群标题，请检查 bot 帐号权限");
    }
    store.save_group(&group).await?;
    let reply = format!("时区已变更至：{}", group.timezone);
    info!("Replied: {:?}", reply);
    return_message(&m, reply)
}

pub async fn push(m: Message, env: Env, bot: Bot<'_>) -> Result<Response, WorkerError> {
    let group_title = get_group_title(&m.chat.kind);
    if group_title.is_none() {
        return warn_group_only(&m);
    }
    let command = m.text.clone().unwrap();
    let new_template_segment = command.split_once(' ');
    if new_template_segment.is_none() {
        return return_message(&m, "无效命令，没有发现新的标题片段");
    }
    let new_template_segment = new_template_segment.unwrap().1;
    let kv = bot.get_kv(&env)?;
    let store = DataStore::new(&kv);
    let mut group = store.load_group_or_create(&m.chat.id, &m.chat.kind).await;

    if !check_permission(&group, &m, &bot).await? {
        return Response::empty();
    }

    group.push_title_template(new_template_segment);
    update_template(&store, &mut group, &bot, &m).await
}

pub async fn push_front(m: Message, env: Env, bot: Bot<'_>) -> Result<Response, WorkerError> {
    let group_title = get_group_title(&m.chat.kind);
    if group_title.is_none() {
        return warn_group_only(&m);
    }
    let command = m.text.clone().unwrap();
    let new_template_segment = command.split_once(' ');
    if new_template_segment.is_none() {
        return return_message(&m, "无效命令，没有发现新的标题片段");
    }
    let new_template_segment = new_template_segment.unwrap().1;
    let kv = bot.get_kv(&env)?;
    let store = DataStore::new(&kv);
    let mut group = store.load_group_or_create(&m.chat.id, &m.chat.kind).await;

    if !check_permission(&group, &m, &bot).await? {
        return Response::empty();
    }

    group.push_front_title_template(new_template_segment);
    update_template(&store, &mut group, &bot, &m).await
}

pub async fn pop(m: Message, env: Env, bot: Bot<'_>) -> Result<Response, WorkerError> {
    let group_title = get_group_title(&m.chat.kind);
    if group_title.is_none() {
        return warn_group_only(&m);
    }
    let kv = bot.get_kv(&env)?;
    let store = DataStore::new(&kv);
    let mut group = store.load_group_or_create(&m.chat.id, &m.chat.kind).await;

    if !check_permission(&group, &m, &bot).await? {
        return Response::empty();
    }

    group.pop_title_template();
    update_template(&store, &mut group, &bot, &m).await
}

pub async fn pop_front(m: Message, env: Env, bot: Bot<'_>) -> Result<Response, WorkerError> {
    let group_title = get_group_title(&m.chat.kind);
    if group_title.is_none() {
        return warn_group_only(&m);
    }
    let kv = bot.get_kv(&env)?;
    let store = DataStore::new(&kv);
    let mut group = store.load_group_or_create(&m.chat.id, &m.chat.kind).await;

    if !check_permission(&group, &m, &bot).await? {
        return Response::empty();
    }

    group.pop_front_title_template();
    update_template(&store, &mut group, &bot, &m).await
}

#[event(scheduled)]
pub async fn handle_scheduled(_req: ScheduledEvent, env: Env, _ctx: ScheduleContext) {
    worker_logger::init_with_string("info");
    set_panic_hook();

    let bot = Bot::new_with_env(&env, DEFAULT_SECRET_TOKEN, VAR_USERNAME, VAR_KV_STORE)
        .expect("Unable to instantiate bot");
    let kv = bot.get_kv(&env).expect("Unable to get KVStore");
    let store = DataStore::new(&kv);
    let groups = store
        .get_group_keys()
        .await
        .expect("Unable to get group keys");
    let date = Date::now();
    for group_name in groups {
        let chat_id: Result<i64, ParseIntError> = group_name.parse();
        if chat_id.is_err() {
            info!("Group ID {} is invalid, skipping...", group_name);
            continue;
        }
        let chat_id = ChatId(chat_id.unwrap());
        let mut group = store
            .load_group(&chat_id)
            .await
            .expect("Unable to load group information");
        if !group.enable {
            info!("Group {} is disabled, skipping...", group_name);
            continue;
        }
        let _res = group.apply_template(&bot, &date).await;
        info!("Title for group {} updated successfully", group_name);
    }
}

pub async fn main_inner(
    req: Request,
    env: Env,
    _ctx: worker::Context,
) -> Result<Response, WorkerError> {
    worker_logger::init_with_string("info");
    log_request(&req);
    set_panic_hook();

    // Bot
    let mut bot = Bot::new_with_env(&env, DEFAULT_SECRET_TOKEN, VAR_USERNAME, VAR_KV_STORE)?;
    bot.register_command("echo", echo);
    bot.register_command("start", start);
    bot.register_command("status", status);
    bot.register_command("enable", enable);
    bot.register_command("disable", disable);
    bot.register_command("set_template", set_template);
    bot.register_command("set_delimiter", set_delimiter);
    bot.register_command("set_timezone", set_timezone);
    bot.register_command("push", push);
    bot.register_command("push_front", push_front);
    bot.register_command("pop", pop);
    bot.register_command("pop_front", pop_front);

    // Router
    let router = Router::with_data(bot).get_async("/", |req, ctx| async move {
        let bot = ctx.data;
        let target = format!("{}updates", req.url()?);
        info!("Setting up webhook, URL: {}", target);
        bot.setup_webhook(target).await?;
        Response::from_json(&bot.get_me().await?)
    });
    let router = router.post_async("/updates", |mut req, ctx| async move {
        Bot::process_update(&mut req, ctx).await
    });

    // Run
    router.run(req, env).await
}

#[event(fetch)]
pub async fn main(req: Request, env: Env, ctx: worker::Context) -> Result<Response, WorkerError> {
    match main_inner(req, env, ctx).await {
        Ok(res) => Ok(res),
        Err(e) => {
            error!("Error occurred: {}", e);
            Ok(Response::from_html("Internal Server Error").expect("Bruh, what just happened?"))
        }
    }
}
