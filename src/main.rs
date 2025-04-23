use std::fs::File;
use std::io::Read;
use std::sync::mpsc::{channel, Receiver};
use std::thread;
use chrono::{NaiveDate, Local, TimeZone};
use eframe::{egui, App, Frame};
use egui_extras::{Column, TableBuilder};
use crate::event_log::{EventRecord, list_event_logs, query_events};
use evtx::EvtxParser;
use csv::ReaderBuilder;
use quick_xml::events::Event as XmlEvent;

mod event_log;

#[derive(Default)]
struct Filters {
    levels: Vec<String>,
    source: String,
    event_id: Option<u16>,
    user: String,
    computer: String,
    keyword: String,
    date_from: Option<NaiveDate>,
    date_to: Option<NaiveDate>,
}

enum SortBy { Time, Level, EventID, Source }

#[derive(Clone, Copy, PartialEq, Eq)]
enum ThemeMode {
    System,
    GruvboxDark,
    GruvboxLight,
    SolarizedDark,
    SolarizedLight,
    Arc,
    Dracula,
    Nord,
}

struct EventViewerApp {
    all_events: Vec<EventRecord>,
    filtered_events: Vec<EventRecord>,
    filters: Filters,
    sort_by: SortBy,
    sort_desc: bool,
    selected: Option<usize>,
    recv: Receiver<EventRecord>,
    paused: bool,
    page_size: u32,
    current_page: u32,
    available_logs: Vec<String>,
    selected_logs: Vec<String>,
    theme_mode: ThemeMode,
}

impl Default for EventViewerApp {
    fn default() -> Self {
        let available_logs = list_event_logs();
        let selected_logs = available_logs.clone();
        let (tx, rx) = channel();
        let available_logs_for_thread = available_logs.clone();
        // spawn polling thread
        thread::spawn(move || {
            loop {
                // simple polling: query newest 50
                let events = query_events(&available_logs_for_thread.join(","), 50);
                for ev in events.into_iter().rev() {
                    let _ = tx.send(ev);
                }
                std::thread::sleep(std::time::Duration::from_secs(2));
            }
        });
        let mut app = Self {
            all_events: vec![],
            filtered_events: vec![],
            filters: Filters::default(),
            sort_by: SortBy::Time,
            sort_desc: true,
            selected: None,
            recv: rx,
            paused: false,
            page_size: 100,
            current_page: 0,
            available_logs,
            selected_logs,
            theme_mode: ThemeMode::System,
        };
        app.refresh_page();
        app
    }
}

impl EventViewerApp {
    fn refresh_page(&mut self) {
        self.current_page = 0;
        self.all_events = self.selected_logs.iter().flat_map(|log| query_events(log, self.page_size)).collect();
        self.apply_filters();
    }

    fn apply_filters(&mut self) {
        let mut evs = self.all_events.clone();
        // basic filters
        evs.retain(|e| {
            (self.filters.levels.is_empty() || self.filters.levels.contains(&e.level)) &&
            (self.filters.source.is_empty() || e.source.contains(&self.filters.source)) &&
            (self.filters.event_id.map_or(true, |id| e.event_id == id)) &&
            (self.filters.user.is_empty() || e.user.contains(&self.filters.user)) &&
            (self.filters.computer.is_empty() || e.computer.contains(&self.filters.computer)) &&
            (self.filters.keyword.is_empty() || e.description.contains(&self.filters.keyword) || e.raw_xml.contains(&self.filters.keyword)) &&
            (self.filters.date_from.map_or(true, |d| e.time_created.date_naive() >= d)) &&
            (self.filters.date_to.map_or(true, |d| e.time_created.date_naive() <= d))
        });
        // Always sort by time descending (most recent first)
        evs.sort_by(|a, b| b.time_created.timestamp().cmp(&a.time_created.timestamp()));
        self.filtered_events = evs;
    }

    fn update_live(&mut self) {
        if !self.paused {
            while let Ok(ev) = self.recv.try_recv() {
                self.all_events.insert(0, ev);
            }
            self.apply_filters();
        }
    }

    pub fn import_file(&mut self, path: &str) {
        self.paused = true; // Pause polling when importing
        if path.ends_with(".evtx") {
            if let Ok(mut parser) = EvtxParser::from_path(path) {
                self.filtered_events.clear();
                for record in parser.records_json() {
                    if let Ok(json) = record {
                        let description = format!("{:?}", json);
                        self.filtered_events.push(EventRecord {
                            log_name: "Imported EVTX".to_string(),
                            time_created: chrono::Local::now(),
                            event_id: 0,
                            level: "Info".to_string(),
                            source: "Import".to_string(),
                            user: String::new(),
                            computer: String::new(),
                            description: description.chars().take(200).collect(),
                            raw_xml: description,
                        });
                    }
                }
            }
        } else if path.ends_with(".xml") {
            let mut file = File::open(path).unwrap();
            let mut contents = String::new();
            file.read_to_string(&mut contents).unwrap();
            self.filtered_events.clear();
            let mut reader = quick_xml::Reader::from_str(&contents);
            reader.trim_text(true);
            let mut buf = Vec::new();
            let mut in_event = false;
            let mut event_xml = String::new();
            let mut fields = EventRecord {
                log_name: "Imported XML".to_string(),
                time_created: chrono::Local::now(),
                event_id: 0,
                level: String::new(),
                source: String::new(),
                user: String::new(),
                computer: String::new(),
                description: String::new(),
                raw_xml: String::new(),
            };
            loop {
                match reader.read_event_into(&mut buf) {
                    Ok(XmlEvent::Start(ref e)) if e.name().as_ref() == b"Event" => {
                        in_event = true;
                        event_xml.clear();
                        event_xml.push_str("<Event>");
                        fields = EventRecord {
                            log_name: "Imported XML".to_string(),
                            time_created: chrono::Local::now(),
                            event_id: 0,
                            level: String::new(),
                            source: String::new(),
                            user: String::new(),
                            computer: String::new(),
                            description: String::new(),
                            raw_xml: String::new(),
                        };
                    }
                    Ok(XmlEvent::End(ref e)) if e.name().as_ref() == b"Event" => {
                        in_event = false;
                        event_xml.push_str("</Event>");
                        // Store the full XML for this event, including all nested tags and text
                        fields.raw_xml = event_xml.clone();
                        self.filtered_events.push(fields.clone());
                    }
                    Ok(XmlEvent::Text(e)) if in_event => {
                        event_xml.push_str(&e.unescape().unwrap_or_default());
                    }
                    Ok(XmlEvent::CData(e)) if in_event => {
                        event_xml.push_str(&String::from_utf8_lossy(&e.into_inner()));
                    }
                    Ok(XmlEvent::Start(ref e)) if in_event => {
                        let tag_buf = String::from_utf8_lossy(e.name().as_ref()).to_string();
                        let tag = &tag_buf;
                        event_xml.push('<');
                        event_xml.push_str(tag);
                        // Write all attributes
                        for attr in e.attributes().with_checks(false) {
                            if let Ok(attr) = attr {
                                event_xml.push(' ');
                                event_xml.push_str(&String::from_utf8_lossy(attr.key.as_ref()));
                                event_xml.push_str("=\"");
                                event_xml.push_str(&attr.unescape_value().unwrap_or_default());
                                event_xml.push('"');
                            }
                        }
                        event_xml.push('>');
                        // Extract fields from known tags
                        if tag == "TimeCreated" {
                            if let Some(Ok(attr)) = e.attributes().with_checks(false).find(|a| a.as_ref().map(|a| a.key.as_ref() == b"SystemTime").unwrap_or(false)) {
                                if let Ok(val) = attr.unescape_value() {
                                    // Try RFC3339 first, then fallback to space-separated format
                                    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&val) {
                                        fields.time_created = dt.with_timezone(&chrono::Local);
                                    } else if let Ok(ndt) = chrono::NaiveDateTime::parse_from_str(&val, "%Y-%m-%d %H:%M:%S%.f") {
                                        fields.time_created = match chrono::Local.from_local_datetime(&ndt) {
                                            chrono::LocalResult::Single(dt) => dt,
                                            _ => chrono::Local.timestamp(0, 0),
                                        };
                                    }
                                }
                            }
                        } else if tag == "EventID" {
                            if let Ok(XmlEvent::Text(eid)) = reader.read_event_into(&mut buf) {
                                if let Ok(val) = eid.unescape() {
                                    fields.event_id = val.parse().unwrap_or(0);
                                }
                            }
                        } else if tag == "Level" {
                            if let Ok(XmlEvent::Text(lvl)) = reader.read_event_into(&mut buf) {
                                if let Ok(val) = lvl.unescape() {
                                    fields.level = val.to_string();
                                }
                            }
                        } else if tag == "Provider" {
                            for attr in e.attributes().with_checks(false) {
                                if let Ok(attr) = attr {
                                    if attr.key.as_ref() == b"Name" {
                                        fields.source = attr.unescape_value().unwrap_or_default().to_string();
                                    }
                                }
                            }
                        } else if tag == "Computer" {
                            if let Ok(XmlEvent::Text(comp)) = reader.read_event_into(&mut buf) {
                                if let Ok(val) = comp.unescape() {
                                    fields.computer = val.to_string();
                                }
                            }
                        } else if tag == "UserID" {
                            if let Ok(XmlEvent::Text(user)) = reader.read_event_into(&mut buf) {
                                if let Ok(val) = user.unescape() {
                                    fields.user = val.to_string();
                                }
                            }
                        } else if tag == "Data" {
                            if let Ok(XmlEvent::Text(desc)) = reader.read_event_into(&mut buf) {
                                if let Ok(val) = desc.unescape() {
                                    if !fields.description.is_empty() {
                                        fields.description.push_str("; ");
                                    }
                                    fields.description.push_str(&val);
                                }
                            }
                        }
                    }
                    Ok(XmlEvent::End(ref e)) if in_event => {
                        event_xml.push_str("</");
                        event_xml.push_str(&String::from_utf8_lossy(e.name().as_ref()));
                        event_xml.push('>');
                    }
                    Ok(XmlEvent::Eof) => break,
                    Err(_) => break,
                    _ => {}
                }
                buf.clear();
            }
        } else if path.ends_with(".csv") {
            let file = std::fs::File::open(path);
            if let Ok(file) = file {
                let mut rdr = ReaderBuilder::new().has_headers(true).from_reader(file);
                self.filtered_events.clear();
                for result in rdr.records() {
                    if let Ok(record) = result {
                        let description = record.iter().collect::<Vec<_>>().join(", ");
                        self.filtered_events.push(EventRecord {
                            log_name: "Imported CSV".to_string(),
                            time_created: chrono::Local::now(),
                            event_id: 0,
                            level: "Info".to_string(),
                            source: "Import".to_string(),
                            user: String::new(),
                            computer: String::new(),
                            description: description.chars().take(200).collect(),
                            raw_xml: description,
                        });
                    }
                }
            }
        }
    }
}

impl App for EventViewerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut Frame) {
        match self.theme_mode {
            ThemeMode::System => {}, // Use default
            ThemeMode::GruvboxDark => {
                ctx.set_visuals(egui::Visuals::dark());
                ctx.set_style(egui::Style {
                    visuals: egui::Visuals {
                        window_fill: egui::Color32::from_rgb(40, 40, 40),
                        panel_fill: egui::Color32::from_rgb(29, 32, 33),
                        faint_bg_color: egui::Color32::from_rgb(60, 56, 54),
                        extreme_bg_color: egui::Color32::from_rgb(29, 32, 33),
                        ..egui::Visuals::dark()
                    },
                    ..egui::Style::default()
                });
            },
            ThemeMode::GruvboxLight => {
                ctx.set_visuals(egui::Visuals::light());
                ctx.set_style(egui::Style {
                    visuals: egui::Visuals {
                        window_fill: egui::Color32::from_rgb(251, 241, 199),
                        panel_fill: egui::Color32::from_rgb(235, 219, 178),
                        faint_bg_color: egui::Color32::from_rgb(213, 196, 161),
                        extreme_bg_color: egui::Color32::from_rgb(235, 219, 178),
                        ..egui::Visuals::light()
                    },
                    ..egui::Style::default()
                });
            },
            ThemeMode::SolarizedDark => {
                ctx.set_visuals(egui::Visuals::dark());
                ctx.set_style(egui::Style {
                    visuals: egui::Visuals {
                        window_fill: egui::Color32::from_rgb(0, 43, 54),
                        panel_fill: egui::Color32::from_rgb(7, 54, 66),
                        faint_bg_color: egui::Color32::from_rgb(88, 110, 117),
                        extreme_bg_color: egui::Color32::from_rgb(7, 54, 66),
                        ..egui::Visuals::dark()
                    },
                    ..egui::Style::default()
                });
            },
            ThemeMode::SolarizedLight => {
                ctx.set_visuals(egui::Visuals::light());
                ctx.set_style(egui::Style {
                    visuals: egui::Visuals {
                        window_fill: egui::Color32::from_rgb(253, 246, 227),
                        panel_fill: egui::Color32::from_rgb(238, 232, 213),
                        faint_bg_color: egui::Color32::from_rgb(147, 161, 161),
                        extreme_bg_color: egui::Color32::from_rgb(238, 232, 213),
                        ..egui::Visuals::light()
                    },
                    ..egui::Style::default()
                });
            },
            ThemeMode::Arc => {
                ctx.set_visuals(egui::Visuals::light());
                ctx.set_style(egui::Style {
                    visuals: egui::Visuals {
                        window_fill: egui::Color32::from_rgb(238, 241, 245),
                        panel_fill: egui::Color32::from_rgb(220, 224, 230),
                        faint_bg_color: egui::Color32::from_rgb(200, 204, 210),
                        extreme_bg_color: egui::Color32::from_rgb(220, 224, 230),
                        ..egui::Visuals::light()
                    },
                    ..egui::Style::default()
                });
            },
            ThemeMode::Dracula => {
                ctx.set_visuals(egui::Visuals::dark());
                ctx.set_style(egui::Style {
                    visuals: egui::Visuals {
                        window_fill: egui::Color32::from_rgb(40, 42, 54),
                        panel_fill: egui::Color32::from_rgb(68, 71, 90),
                        faint_bg_color: egui::Color32::from_rgb(98, 114, 164),
                        extreme_bg_color: egui::Color32::from_rgb(68, 71, 90),
                        ..egui::Visuals::dark()
                    },
                    ..egui::Style::default()
                });
            },
            ThemeMode::Nord => {
                ctx.set_visuals(egui::Visuals::dark());
                ctx.set_style(egui::Style {
                    visuals: egui::Visuals {
                        window_fill: egui::Color32::from_rgb(46, 52, 64),
                        panel_fill: egui::Color32::from_rgb(59, 66, 82),
                        faint_bg_color: egui::Color32::from_rgb(76, 86, 106),
                        extreme_bg_color: egui::Color32::from_rgb(59, 66, 82),
                        ..egui::Visuals::dark()
                    },
                    ..egui::Style::default()
                });
            },
        }

        self.update_live();
        egui::TopBottomPanel::top("controls").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Logs:");
                for log in &self.available_logs {
                    let mut sel = self.selected_logs.contains(log);
                    ui.checkbox(&mut sel, log);
                    if sel && !self.selected_logs.contains(log) {
                        self.selected_logs.push(log.clone());
                    } else if !sel {
                        self.selected_logs.retain(|l| l != log);
                    }
                }
                if ui.button("Refresh").clicked() { self.refresh_page(); }
                if ui.button(if self.paused { "Resume" } else { "Pause" }).clicked() {
                    self.paused = !self.paused;
                }
                if ui.button("Import File").clicked() {
                    if let Some(path) = rfd::FileDialog::new().add_filter("Event Files", &["evtx", "xml", "csv"]).pick_file() {
                        if let Some(path_str) = path.to_str() {
                            self.import_file(path_str);
                            if !self.filtered_events.is_empty() {
                                self.selected = Some(0);
                            }
                        }
                    }
                }
                ui.separator();
                ui.label("Theme:");
                egui::ComboBox::from_id_source("theme_mode").selected_text(match self.theme_mode {
                    ThemeMode::System => "System",
                    ThemeMode::GruvboxDark => "Gruvbox Dark",
                    ThemeMode::GruvboxLight => "Gruvbox Light",
                    ThemeMode::SolarizedDark => "Solarized Dark",
                    ThemeMode::SolarizedLight => "Solarized Light",
                    ThemeMode::Arc => "Arc-Theme",
                    ThemeMode::Dracula => "Dracula",
                    ThemeMode::Nord => "Nord",
                }).show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.theme_mode, ThemeMode::System, "System");
                    ui.selectable_value(&mut self.theme_mode, ThemeMode::GruvboxDark, "Gruvbox Dark");
                    ui.selectable_value(&mut self.theme_mode, ThemeMode::GruvboxLight, "Gruvbox Light");
                    ui.selectable_value(&mut self.theme_mode, ThemeMode::SolarizedDark, "Solarized Dark");
                    ui.selectable_value(&mut self.theme_mode, ThemeMode::SolarizedLight, "Solarized Light");
                    ui.selectable_value(&mut self.theme_mode, ThemeMode::Arc, "Arc-Theme");
                    ui.selectable_value(&mut self.theme_mode, ThemeMode::Dracula, "Dracula");
                    ui.selectable_value(&mut self.theme_mode, ThemeMode::Nord, "Nord");
                });
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::both().show(ui, |ui| {
                TableBuilder::new(ui)
                    .column(Column::auto().resizable(true)) // Time
                    .column(Column::initial(60.0)) // Level
                    .column(Column::initial(60.0)) // EventID
                    .column(Column::initial(100.0)) // Source
                    .column(Column::initial(120.0)) // Username
                    .column(Column::initial(180.0)) // Computer
                    .striped(true)
                    .resizable(true)
                    .header(20.0, |mut header| {
                        header.col(|ui| { ui.label("Time"); });
                        header.col(|ui| { ui.label("Level"); });
                        header.col(|ui| { ui.label("ID"); });
                        header.col(|ui| { ui.label("Source"); });
                        header.col(|ui| { ui.label("Username"); });
                        header.col(|ui| { ui.label("Computer"); });
                    })
                    .body(|body| {
                        body.rows(20.0, self.filtered_events.len(), |row_index, mut row| {
                            let ev = &self.filtered_events[row_index];
                            let selected = self.selected == Some(row_index);
                            let response = row.col(|ui| {
                                let label = ui.selectable_label(selected, ev.time_created.format("%Y-%m-%d %H:%M:%S").to_string());
                                if label.clicked() {
                                    self.selected = Some(row_index);
                                }
                            });
                            row.col(|ui| { ui.label(&ev.level); });
                            row.col(|ui| { ui.label(ev.event_id.to_string()); });
                            row.col(|ui| { ui.label(&ev.source); });
                            row.col(|ui| { ui.label(&ev.user); }); // Now Username
                            row.col(|ui| { ui.label(&ev.computer); });
                        });
                    });
            });
        });
        egui::SidePanel::right("details").resizable(true).show(ctx, |ui| {
            egui::ScrollArea::both().show(ui, |ui| {
                ui.heading("Event Details");
                if let Some(ev) = self.filtered_events.get(self.selected.unwrap_or(0)) {
                    ui.label(format!("Log: {}", ev.log_name));
                    ui.separator();
                    ui.label(format!("Time: {}", ev.time_created));
                    ui.label(format!("Level: {}", ev.level));
                    ui.label(format!("Event ID: {}", ev.event_id));
                    ui.label(format!("Source: {}", ev.source));
                    ui.label(format!("Username: {}", ev.user));
                    ui.label(format!("Computer: {}", ev.computer));
                    ui.separator();
                    ui.collapsing("Description", |ui| { ui.label(&ev.description); });
                    ui.collapsing("Raw XML", |ui| { ui.code(&ev.raw_xml); });
                } else {
                    ui.label("Select an event to see details");
                }
            });
        });
    }
}

fn main() {
    let options = eframe::NativeOptions::default();
    eframe::run_native(
        "Rust Windows Event Viewer",
        options,
        Box::new(|_cc| Box::new(EventViewerApp::default())),
    );
}
