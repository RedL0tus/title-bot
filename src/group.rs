use chrono::{DateTime, Datelike, NaiveDateTime, TimeZone};
use chrono_tz::Tz;
use log::info;
use new_string_template::template::Template;
use serde::{Deserialize, Serialize};
use telegram_types::bot::methods::{ChatTarget, Method, TelegramResult};
use telegram_types::bot::types::{ChatId, ChatType};
use worker::kv::KvStore;
use worker::{Date, Error as WorkerError, Method as RequestMethod};

use super::bot::Bot;

use std::collections::HashMap;

const SET_CHAT_TITLE_FAILED: TelegramResult<bool> = TelegramResult {
    ok: false,
    description: None,
    error_code: Some(400),
    result: None,
    parameters: None,
};

macro_rules! add_specifier {
    ($hashmap:ident, $datetime:ident, $specifier:expr) => {
        $hashmap.insert(
            $specifier,
            $datetime.format(&format!("%{}", $specifier)).to_string(),
        );
    };
    ($hashmap:ident, $datetime:ident, $($specifier:expr),+) => {
        $(
            add_specifier!($hashmap, $datetime, $specifier);
        )+
    };
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SetChatTitle<'a> {
    pub chat_id: ChatTarget<'a>,
    pub title: &'a str,
}

#[derive(Clone, Debug)]
pub struct TemplateContext<'a> {
    inner: HashMap<&'a str, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Group {
    pub enable: bool,
    pub id: ChatId,
    pub title_segment: Vec<String>,
    pub delimiter: String,
    pub last_title: String,
    pub timezone: String,
    pub require_admin: bool,
}

#[derive(Clone)]
pub struct DataStore<'a> {
    kv: &'a KvStore,
}

pub fn get_group_title(chat: &ChatType) -> Option<&str> {
    match chat {
        ChatType::Group { title, .. } => Some(title),
        ChatType::Supergroup { title, .. } => Some(title),
        ChatType::Channel { title, .. } => Some(title),
        _ => None,
    }
}

pub fn get_raw_chat_id(chat_id: &ChatId) -> i64 {
    let ChatId(raw_id) = *chat_id;
    raw_id
}

impl<'a> Method for SetChatTitle<'a> {
    const NAME: &'static str = "setChatTitle";
    type Item = bool;
}

impl<'a> TemplateContext<'a> {
    pub fn generate(datetime: DateTime<Tz>) -> Self {
        let mut ret = HashMap::new();
        add_specifier!(
            ret, datetime, "Y", "C", "y", "m", "b", "B", "h", "d", "e", "a", "A", "w", "u", "U",
            "W", "G", "g", "V", "j", "D", "x", "F", "v", "H", "k", "I", "l", "P", "p", "M", "S",
            "f", "R", "T", "X", "r", "Z", "z", ":z", "c", "+", "s"
        );
        ret.insert("yeshu", (datetime.date().year() - 1988).to_string());
        Self { inner: ret }
    }
}

impl<'a> From<TemplateContext<'a>> for HashMap<&'a str, String> {
    fn from(context: TemplateContext<'a>) -> HashMap<&'a str, String> {
        context.inner
    }
}

impl Group {
    pub fn new(chat_id: &ChatId, chat_type: &ChatType) -> Self {
        let title = get_group_title(chat_type);
        let title_str = title.unwrap().to_string();
        Self {
            enable: false,
            id: *chat_id,
            title_segment: vec![title_str.clone()],
            delimiter: " | ".to_string(),
            last_title: title_str,
            timezone: Tz::UTC.to_string(),
            require_admin: true,
        }
    }

    pub fn push_title_template<S: AsRef<str>>(&mut self, new_segment: S) {
        self.title_segment.push(new_segment.as_ref().to_string());
    }

    pub fn push_front_title_template<S: AsRef<str>>(&mut self, new_segment: S) {
        self.title_segment
            .insert(0, new_segment.as_ref().to_string());
    }

    pub fn pop_title_template(&mut self) {
        if self.title_segment.len() > 1 {
            self.title_segment.pop();
        }
    }

    pub fn pop_front_title_template(&mut self) {
        if self.title_segment.len() > 1 {
            self.title_segment.drain(0..1);
        }
    }

    pub fn get_time(&self, time: NaiveDateTime) -> DateTime<Tz> {
        let tz: Tz = self.timezone.parse().unwrap_or(Tz::UTC);
        DateTime::from_utc(time, tz.offset_from_utc_datetime(&time))
    }

    pub fn get_last_title(&self) -> &str {
        &self.last_title
    }

    pub fn join_title_template(&self) -> String {
        self.title_segment.join(&format!(" {} ", self.delimiter))
    }

    pub fn clear_title_template(&mut self) {
        self.title_segment.clear();
    }

    pub fn get_new_title<S: AsRef<str>>(
        &self,
        context: &HashMap<&str, S>,
    ) -> Result<String, WorkerError> {
        let template = Template::new(self.join_title_template());
        template
            .render(context)
            .map_err(|e| WorkerError::RustError(e.to_string()))
    }

    pub async fn update_title<S: AsRef<str>>(
        &self,
        bot: &Bot<'_>,
        title: S,
    ) -> Result<bool, WorkerError> {
        let set_chat_title = SetChatTitle {
            chat_id: ChatTarget::Id(self.id),
            title: title.as_ref(),
        };
        let response = bot
            .send_json_request(set_chat_title, RequestMethod::Post)
            .await;
        match response {
            Ok(mut res) => Ok(res
                .json::<TelegramResult<bool>>()
                .await
                .unwrap_or(SET_CHAT_TITLE_FAILED)
                .ok),
            Err(e) => Err(e),
        }
    }

    pub async fn apply_template(
        &mut self,
        bot: &Bot<'_>,
        date: &Date,
    ) -> Result<bool, WorkerError> {
        let naive_date = NaiveDateTime::from_timestamp((date.as_millis() / 1000) as i64, 0);
        info!("Got naive time: {}", naive_date);
        let local_time = self.get_time(naive_date);
        info!("Local time: {}", local_time);
        let context = TemplateContext::generate(local_time);
        info!("Generated context: {:?}", context);
        let new_title = self.get_new_title(&HashMap::from(context))?;
        let title_template_length = new_title.len();
        if !(1..=255).contains(&title_template_length) {
            return Err(WorkerError::RustError("Invalid title length".to_string()));
        }
        info!("Applying title: {}", new_title);
        self.update_title(bot, &new_title).await?;
        self.last_title = new_title;
        Ok(true)
    }
}

impl<'a> DataStore<'a> {
    pub fn new(kv: &'a KvStore) -> Self {
        Self { kv }
    }

    pub async fn get_group_keys(&self) -> Result<Vec<String>, WorkerError> {
        let list_result = self
            .kv
            .list()
            .prefix("group-".to_string())
            .execute()
            .await?;
        let mut ret: Vec<String> = list_result
            .keys
            .into_iter()
            .map(|k| k.name[6..].to_string())
            .collect();
        let mut cursor = list_result.cursor;
        let mut complete = list_result.list_complete;
        while (!complete) && cursor.is_some() {
            let list_result = self
                .kv
                .list()
                .prefix("group-".to_string())
                .cursor(cursor.unwrap())
                .execute()
                .await?;
            let mut new_result: Vec<String> = list_result
                .keys
                .into_iter()
                .map(|k| k.name[6..].to_string())
                .collect();
            ret.append(&mut new_result);
            cursor = list_result.cursor;
            complete = list_result.list_complete;
        }
        Ok(ret)
    }

    pub async fn load_group(&self, id: &ChatId) -> Result<Group, WorkerError> {
        let raw_id = get_raw_chat_id(id);
        let key = format!("group-{}", raw_id);
        let data =
            self.kv.get(&key).bytes().await?.ok_or_else(|| {
                WorkerError::RustError("Group info not found in KvStore".to_string())
            })?;
        bincode::deserialize(&data).map_err(|e| WorkerError::RustError(e.to_string()))
    }

    pub async fn load_group_or_create(&self, id: &ChatId, chat_type: &ChatType) -> Group {
        let stored_group = self.load_group(id).await;
        if let Ok(group) = stored_group {
            group
        } else {
            let new_group = Group::new(id, chat_type);
            self.save_group(&new_group).await.ok();
            new_group
        }
    }

    pub async fn save_group(&self, group: &Group) -> Result<(), WorkerError> {
        let raw_id = get_raw_chat_id(&group.id);
        let key = format!("group-{}", raw_id);
        let data = bincode::serialize(&group).map_err(|e| WorkerError::RustError(e.to_string()))?;
        Ok(self.kv.put_bytes(&key, &data)?.execute().await?)
    }
}
