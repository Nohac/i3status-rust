use std::convert::{TryFrom, TryInto};
use std::io::{self, Cursor, ErrorKind};
use std::ops::{Add, Sub};

use chrono::{DateTime, Duration, NaiveTime, Utc};
use crossbeam_channel::Sender;
use serde::Serialize;
use serde_derive::Deserialize;

use crate::blocks::{Block, ConfigBlock, Update};
use crate::config::SharedConfig;
use crate::de::deserialize_duration;
use crate::errors::*;
use crate::formatting::FormatTemplate;
use crate::scheduler::Task;
use crate::widgets::text::TextWidget;
use crate::widgets::I3BarWidget;

const HEADER: &str = "start_time|title|runner|setup_time|length|category|host";

#[derive(Debug)]
struct Entry {
    start_time: DateTime<Utc>,
    length: Option<Duration>,
    setup_time: Option<Duration>,
    title: String,
    runner: String,
    category: String,
    host: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Record {
    pub start_time: String,
    pub title: String,
    pub runner: String,
    pub setup_time: String,
    pub length: String,
    pub category: String,
    pub host: String,
}

impl TryFrom<Record> for Entry {
    type Error = io::Error;

    fn try_from(r: Record) -> std::result::Result<Self, Self::Error> {
        let start_time = DateTime::parse_from_rfc3339(&r.start_time)
            .map_err(|e| io::Error::new(ErrorKind::InvalidData, e.to_string()))?
            .into();
        let setup_time = match NaiveTime::parse_from_str(&r.setup_time, "%T") {
            Ok(t) => Some(t.sub(NaiveTime::from_hms(0, 0, 0))),
            Err(_) => None,
        };

        let length = match NaiveTime::parse_from_str(&r.length, "%T") {
            Ok(t) => Some(t.sub(NaiveTime::from_hms(0, 0, 0))),
            Err(_) => None,
        };

        Ok(Entry {
            start_time,
            length,
            setup_time,
            title: r.title,
            runner: r.runner,
            category: r.category,
            host: r.host,
        })
    }
}

pub struct GDQ {
    id: usize,
    text: TextWidget,
    format: FormatTemplate,
    update_interval: std::time::Duration,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields, default)]
pub struct GDQConfig {
    /// Update interval in seconds
    #[serde(deserialize_with = "deserialize_duration")]
    pub interval: std::time::Duration,

    /// Format override
    pub format: FormatTemplate,
}

impl Default for GDQConfig {
    fn default() -> Self {
        Self {
            interval: std::time::Duration::from_secs(20),
            format: FormatTemplate::default(),
        }
    }
}

impl ConfigBlock for GDQ {
    type Config = GDQConfig;

    fn new(
        id: usize,
        block_config: Self::Config,
        shared_config: SharedConfig,
        _: Sender<Task>,
    ) -> Result<Self> {
        let text = TextWidget::new(id, 0, shared_config)
            .with_text("N/A")
            .with_icon("joystick")?;
        Ok(GDQ {
            id,
            text,
            format: block_config.format.with_default("{name}")?,
            update_interval: block_config.interval,
        })
    }
}

impl Block for GDQ {
    fn update(&mut self) -> Result<Option<Update>> {
        let r = match ureq::get("https://gamesdonequick.com/schedule").call() {
            Ok(r) => r,
            Err(_) => {
                self.text.set_text("ERR".to_string());
                return Ok(Some(self.update_interval.into()));
            }
        };

        let schedule_html = match r.into_string() {
            Ok(s) => s,
            Err(_) => {
                self.text.set_text("ERR".to_string());
                return Ok(Some(self.update_interval.into()));
            }
        };

        let root = match visdom::Vis::load(&schedule_html) {
            Ok(r) => r,
            Err(_) => {
                self.text.set_text("ERR".to_string());
                return Ok(Some(self.update_interval.into()));
            }
        };
        let list = root.find("#runTable tbody tr");

        let mut list_iter = list.into_iter();
        let mut csv_data: Vec<String> = vec![];
        csv_data.push(HEADER.to_string());

        while let Some(first) = list_iter.next() {
            let second = match list_iter.next() {
                Some(e) => e,
                None => continue,
            };

            let first_delim = first
                .children()
                .into_iter()
                .map(|e| e.text().trim().to_string())
                .collect::<Vec<String>>()
                .join("|");
            let second_delim = second
                .children()
                .into_iter()
                .map(|e| e.text().trim().to_string())
                .collect::<Vec<String>>()
                .join("|");
            let delim = format!("{first_delim}|{second_delim}");
            csv_data.push(delim);
        }

        let csv_string = csv_data.join("\n");
        let csv_file = Cursor::new(csv_string.as_bytes());

        let mut rdr = csv::ReaderBuilder::new()
            .delimiter(b'|')
            .from_reader(csv_file);

        let mut entries = vec![];
        for result in rdr.deserialize() {
            let record: Record = match result {
                Ok(r) => r,
                Err(_) => continue,
            };
            let entry: Entry = match record.try_into() {
                Ok(e) => e,
                Err(_) => continue,
            };
            entries.push(entry);
        }

        let now = Utc::now();
        entries.sort_by(|a, b| a.start_time.partial_cmp(&b.start_time).unwrap());

        let current_index = match entries.iter().position(|e| {
            e.start_time
                .add(e.length.unwrap_or(Duration::seconds(0)))
                .gt(&now)
        }) {
            Some(i) => i,
            None => {
                self.text.set_text("ERR".into());
                return Ok(Some(self.update_interval.into()));
            }
        };
        let current = entries.remove(current_index);
        let next = entries.iter().find(|e| e.start_time.gt(&now));
        self.text.set_text(format!(
            "{} -> {}",
            current.title,
            next.map(|c| c.title.clone()).unwrap_or("None".to_string()),
        ));

        Ok(Some(self.update_interval.into()))
    }

    fn view(&self) -> Vec<&dyn I3BarWidget> {
        vec![&self.text]
    }

    fn id(&self) -> usize {
        self.id
    }
}
