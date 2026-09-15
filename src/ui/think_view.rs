use crate::{i18n::Lang, session::MessageStatus};

pub fn label(status: MessageStatus, lang: Lang) -> &'static str {
    match (lang, status) {
        (Lang::Zh, MessageStatus::Streaming) => "思考中",
        (Lang::Zh, MessageStatus::Completed) => "思考完成",
        (Lang::Zh, MessageStatus::Cancelled) => "思考已中止",
        (Lang::Zh, MessageStatus::Failed) => "思考中断",
        (Lang::En, MessageStatus::Streaming) => "Thinking",
        (Lang::En, MessageStatus::Completed) => "Thought complete",
        (Lang::En, MessageStatus::Cancelled) => "Thought cancelled",
        (Lang::En, MessageStatus::Failed) => "Thought interrupted",
    }
}

pub fn content(text: &str) -> super::tool_view::DetailContent {
    super::tool_view::DetailContent {
        text: text.to_owned(),
        markdown: true,
    }
}
