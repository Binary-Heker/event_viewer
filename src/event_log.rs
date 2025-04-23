use chrono::{DateTime, Local};
use quick_xml::Reader;
use quick_xml::events::Event as XmlEvent;
use std::process::Command;
use std::fs::File;
use std::io::{BufRead, BufReader};

#[derive(Clone, Debug)]
pub struct EventRecord {
    pub log_name: String,
    pub time_created: DateTime<Local>,
    pub event_id: u16,
    pub level: String,
    pub source: String,
    pub user: String,
    pub computer: String,
    pub description: String,
    pub raw_xml: String,
}

pub fn list_event_logs() -> Vec<String> {
    #[cfg(target_os = "windows")]
    {
        vec![
            "Application".to_string(),
            "Security".to_string(),
            "System".to_string(),
            "Setup".to_string(),
        ]
    }
    #[cfg(not(target_os = "windows"))]
    {
        vec!["system".to_string()]
    }
}

pub fn query_events(log: &str, max_records: u32) -> Vec<EventRecord> {
    #[cfg(target_os = "windows")]
    {
        let args = ["qe", log, "/f:xml", &format!("/c:{}", max_records), "/rd:true"];
        let output = Command::new("wevtutil")
            .args(&args)
            .output()
            .unwrap_or_else(|e| {
                eprintln!("Failed to execute wevtutil: {}", e);
                std::process::exit(1);
            });
        if !output.status.success() {
            eprintln!(
                "wevtutil qe error: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            return Vec::new();
        }
        let xml = String::from_utf8_lossy(&output.stdout);
        let mut events = Vec::new();
        for raw in xml.split("</Event>") {
            let raw = raw.trim();
            if raw.is_empty() {
                continue;
            }
            let raw_event = format!("{}{}", raw, "</Event>");
            if let Some(ev) = parse_event(&raw_event) {
                events.push(ev);
            }
        }
        events
    }
    #[cfg(not(target_os = "windows"))]
    {
        // For Unix: read from /var/log/system.log (macOS) or /var/log/syslog (Linux)
        let log_path = if cfg!(target_os = "macos") {
            "/var/log/system.log"
        } else {
            "/var/log/syslog"
        };
        let file = File::open(log_path);
        if file.is_err() {
            eprintln!("Failed to open system log: {}", log_path);
            return Vec::new();
        }
        let reader = BufReader::new(file.unwrap());
        let lines: Vec<_> = reader.lines().filter_map(Result::ok).collect();
        let mut events = Vec::new();
        for line in lines.iter().rev().take(max_records as usize) {
            let record = EventRecord {
                log_name: log.to_string(),
                time_created: Local::now(), // Could parse from line if format known
                event_id: 0,
                level: String::new(),
                source: String::new(),
                user: String::new(),
                computer: String::new(),
                description: line.clone(),
                raw_xml: line.clone(),
            };
            events.push(record);
        }
        events
    }
}

/// Parses an individual Event XML into EventRecord
fn parse_event(xml: &str) -> Option<EventRecord> {
    let mut reader = Reader::from_str(xml);
    reader.trim_text(true);
    let mut buf = Vec::new();
    let mut record = EventRecord {
        log_name: String::new(),
        time_created: Local::now(),
        event_id: 0,
        level: String::new(),
        source: String::new(),
        user: String::new(),
        computer: String::new(),
        description: String::new(),
        raw_xml: xml.to_string(),
    };
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(XmlEvent::Start(ref e)) => match e.name().as_ref() {
                b"Provider" => {
                    for attr in e.attributes().with_checks(false) {
                        if let Ok(attr) = attr {
                            if attr.key.as_ref() == b"Name" {
                                record.source = attr.unescape_value().unwrap_or_default().to_string();
                            }
                        }
                    }
                }
                b"EventID" => {
                    if let Ok(XmlEvent::Text(e)) = reader.read_event_into(&mut buf) {
                        let text = e.unescape().unwrap_or_default().to_string();
                        record.event_id = text.parse().unwrap_or(0);
                    }
                }
                b"Level" => {
                    if let Ok(XmlEvent::Text(e)) = reader.read_event_into(&mut buf) {
                        let lvl = e.unescape().unwrap_or_default().to_string();
                        record.level = match lvl.as_str() {
                            "1" => "Critical".into(),
                            "2" => "Error".into(),
                            "3" => "Warning".into(),
                            "4" => "Information".into(),
                            "5" => "Verbose".into(),
                            _ => lvl,
                        };
                    }
                }
                b"TimeCreated" => {
                    for attr in e.attributes().with_checks(false) {
                        if let Ok(attr) = attr {
                            if attr.key.as_ref() == b"SystemTime" {
                                if let Ok(ts) = attr.unescape_value() {
                                    if let Ok(dt) = DateTime::parse_from_rfc3339(&ts) {
                                        record.time_created = dt.with_timezone(&Local);
                                    }
                                }
                            }
                        }
                    }
                }
                b"Computer" => {
                    if let Ok(XmlEvent::Text(e)) = reader.read_event_into(&mut buf) {
                        record.computer = e.unescape().unwrap_or_default().to_string();
                    }
                }
                b"Security" => {
                    for attr in e.attributes().with_checks(false) {
                        if let Ok(attr) = attr {
                            if attr.key.as_ref() == b"UserID" {
                                record.user = attr.unescape_value().unwrap_or_default().to_string();
                            }
                        }
                    }
                }
                b"Data" => {
                    if let Ok(XmlEvent::Text(e)) = reader.read_event_into(&mut buf) {
                        let data = e.unescape().unwrap_or_default().to_string();
                        if !record.description.is_empty() {
                            record.description.push_str("; ");
                        }
                        record.description.push_str(&data);
                    }
                }
                b"Channel" => {
                    if let Ok(XmlEvent::Text(e)) = reader.read_event_into(&mut buf) {
                        record.log_name = e.unescape().unwrap_or_default().to_string();
                    }
                }
                _ => {}
            },
            Ok(XmlEvent::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    Some(record)
}
