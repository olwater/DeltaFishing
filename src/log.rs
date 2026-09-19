//! 环形日志缓冲：时间戳 + 级别 + 文本，供界面展示与导出。

use chrono::Local;
use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Ok,
    Warn,
    Error,
}

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub time: String,
    pub level: Level,
    pub text: String,
}

pub const MAX_LINES: usize = 1000;

#[derive(Debug)]
pub struct LogBuffer {
    entries: VecDeque<LogEntry>,
}

impl Default for LogBuffer {
    fn default() -> Self {
        Self {
            entries: VecDeque::with_capacity(MAX_LINES),
        }
    }
}

impl LogBuffer {
    pub fn push(&mut self, level: Level, text: impl Into<String>) {
        let text = text.into();
        let time = Local::now().format("%H:%M:%S").to_string();
        // 同一秒的多行日志拆行显示更整齐
        for (i, line) in text.lines().enumerate() {
            let t = if i == 0 {
                time.clone()
            } else {
                "        ".to_string()
            };
            self.entries.push_back(LogEntry {
                time: t,
                level,
                text: line.to_string(),
            });
        }
        while self.entries.len() > MAX_LINES {
            self.entries.pop_front();
        }
    }

    pub fn info(&mut self, text: impl Into<String>) {
        self.push(Level::Info, text);
    }
    pub fn ok(&mut self, text: impl Into<String>) {
        self.push(Level::Ok, text);
    }
    pub fn error(&mut self, text: impl Into<String>) {
        self.push(Level::Error, text);
    }

    pub fn iter(&self) -> impl Iterator<Item = &LogEntry> {
        self.entries.iter()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn export(&self) -> String {
        self.entries
            .iter()
            .map(|e| {
                let tag = match e.level {
                    Level::Info => "INFO",
                    Level::Ok => " OK ",
                    Level::Warn => "WARN",
                    Level::Error => "ERR ",
                };
                format!("{} [{}] {}", e.time, tag, e.text)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}