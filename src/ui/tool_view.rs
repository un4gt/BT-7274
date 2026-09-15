//! Human-readable MCP output; protocol envelopes are available in the raw tab.
use crate::{
    i18n::Lang,
    session::{ToolActivity, ToolStatus},
};
use serde_json::Value;

pub struct DetailContent {
    pub text: String,
    pub markdown: bool,
}

pub fn status_label(status: ToolStatus, lang: Lang) -> &'static str {
    match (lang, status) {
        (Lang::Zh, ToolStatus::Preparing) => "准备中",
        (Lang::Zh, ToolStatus::Queued) => "等待执行",
        (Lang::Zh, ToolStatus::Running) => "运行中",
        (Lang::Zh, ToolStatus::Succeeded) => "已完成",
        (Lang::Zh, ToolStatus::Failed) => "失败",
        (Lang::Zh, ToolStatus::Cancelled) => "已取消",
        (Lang::Zh, ToolStatus::Incomplete) => "未完成",
        (Lang::En, ToolStatus::Preparing) => "Preparing",
        (Lang::En, ToolStatus::Queued) => "Queued",
        (Lang::En, ToolStatus::Running) => "Running",
        (Lang::En, ToolStatus::Succeeded) => "Complete",
        (Lang::En, ToolStatus::Failed) => "Failed",
        (Lang::En, ToolStatus::Cancelled) => "Cancelled",
        (Lang::En, ToolStatus::Incomplete) => "Incomplete",
    }
}

pub fn name(tool: &ToolActivity, lang: Lang) -> String {
    if tool.name.is_empty() {
        return match lang {
            Lang::Zh => "工具调用",
            Lang::En => "Tool call",
        }
        .to_owned();
    }
    if tool.server.is_empty() {
        tool.name.clone()
    } else {
        format!("{}/{}", tool.server, tool.name)
    }
}

pub fn content(tool: &ToolActivity, tab: usize, lang: Lang) -> DetailContent {
    if tab == 0 {
        return DetailContent {
            text: tool
                .arguments
                .as_ref()
                .map(pretty)
                .unwrap_or_else(|| tool.arguments_text.clone()),
            markdown: false,
        };
    }
    let Some(output) = &tool.output else {
        return DetailContent {
            text: match lang {
                Lang::Zh => "尚未收到工具结果",
                Lang::En => "No tool result received",
            }
            .to_owned(),
            markdown: false,
        };
    };
    if tab == 2 {
        return DetailContent {
            text: pretty(output),
            markdown: false,
        };
    }
    if let Some(text) = output.as_str() {
        return DetailContent {
            text: text.to_owned(),
            markdown: true,
        };
    }
    if let Some(items) = output.get("content").and_then(Value::as_array) {
        let mut sections = Vec::new();
        for item in items {
            match item.get("type").and_then(Value::as_str).unwrap_or("") {
                "text" => {
                    if let Some(text) = item.get("text").and_then(Value::as_str) {
                        sections.push(text.to_owned());
                    }
                }
                "image" | "audio" => sections.push(format!(
                    "{} · {}",
                    item["type"].as_str().unwrap_or(""),
                    item["mimeType"].as_str().unwrap_or("")
                )),
                "resource" => {
                    let resource = &item["resource"];
                    sections.push(resource["uri"].as_str().unwrap_or("").to_owned());
                    if let Some(text) = resource.get("text").and_then(Value::as_str) {
                        sections.push(text.to_owned());
                    } else if let Some(mime) = resource.get("mimeType").and_then(Value::as_str) {
                        sections.push(mime.to_owned());
                    }
                }
                "resource_link" => sections.push(format!(
                    "{}\n{}",
                    item["name"].as_str().unwrap_or(""),
                    item["uri"].as_str().unwrap_or("")
                )),
                _ => sections.push(pretty(item)),
            }
        }
        if let Some(structured) = output.get("structuredContent") {
            sections.push(format!("~~~~json\n{}\n~~~~", pretty(structured)));
        }
        if !sections.is_empty() {
            return DetailContent {
                text: sections.join("\n\n"),
                markdown: true,
            };
        }
    }
    DetailContent {
        text: pretty(output),
        markdown: false,
    }
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}
