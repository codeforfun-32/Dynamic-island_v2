use std::collections::HashMap;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use slint::{ModelRc, SharedString, VecModel, Weak};
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

use crate::{MainWindow, SourceItem};

/// Player chosen in the menu. None = "Auto" (first opened player).
static SELECTED: Mutex<Option<String>> = Mutex::new(None);
/// Players in the order they were first seen.
static ORDER: Mutex<Vec<String>> = Mutex::new(Vec::new());

#[derive(Clone, Default, PartialEq)]
struct MediaInfo {
    title: String,
    artist: String,
    source: String,
    playing: bool,
    progress: f32,
    has_media: bool,
}

fn init_com() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
}

/// Current time in Windows ticks (100 ns since 1601), same clock the media timeline uses.
fn now_ticks() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| (d.as_nanos() / 100) as i64 + 116_444_736_000_000_000)
        .unwrap_or(0)
}

fn app_id(session: &Session) -> String {
    session.SourceAppUserModelId().map(|h| h.to_string()).unwrap_or_default()
}

/// Turns something like "Spotify.exe" or "chrome.exe" into a readable name.
fn friendly(id: &str) -> String {
    let lower = id.to_lowercase();
    let known = [
        ("spotify", "Spotify"),
        ("chrome", "Chrome"),
        ("msedge", "Edge"),
        ("edge", "Edge"),
        ("firefox", "Firefox"),
        ("brave", "Brave"),
        ("opera", "Opera"),
        ("vlc", "VLC"),
        ("zunemusic", "Media Player"),
        ("applemusic", "Apple Music"),
        ("itunes", "iTunes"),
        ("telegram", "Telegram"),
        ("whatsapp", "WhatsApp"),
    ];
    for (needle, name) in known {
        if lower.contains(needle) {
            return name.to_string();
        }
    }
    let base = id.rsplit(['!', '\\', '/']).next().unwrap_or(id);
    let base = base.trim_end_matches(".exe");
    let mut chars = base.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => "Player".to_string(),
    }
}

/// All open media sessions with a unique key each (app id + number if the app has several).
fn list_sessions() -> Vec<(String, Session)> {
    let Some(manager) = Manager::RequestAsync().ok().and_then(|op| op.get().ok()) else {
        return Vec::new();
    };
    let Ok(sessions) = manager.GetSessions() else { return Vec::new() };

    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut out = Vec::new();
    for session in sessions {
        let id = app_id(&session);
        let n = counts.entry(id.clone()).or_insert(0);
        *n += 1;
        out.push((format!("{id}#{n}"), session));
    }
    out
}

fn update_order(list: &[(String, Session)]) {
    if let Ok(mut order) = ORDER.lock() {
        order.retain(|k| list.iter().any(|(key, _)| key == k));
        for (key, _) in list {
            if !order.contains(key) {
                order.push(key.clone());
            }
        }
    }
}

/// Which session should the island show / control right now?
fn pick_active(list: &[(String, Session)]) -> Option<usize> {
    let selected = SELECTED.lock().ok().and_then(|g| g.clone());
    if let Some(sel) = selected {
        if let Some(i) = list.iter().position(|(k, _)| *k == sel) {
            return Some(i);
        }
        // the chosen player was closed, go back to Auto
        if let Ok(mut s) = SELECTED.lock() {
            *s = None;
        }
    }
    if let Ok(order) = ORDER.lock() {
        for key in order.iter() {
            if let Some(i) = list.iter().position(|(k, _)| k == key) {
                return Some(i);
            }
        }
    }
    if list.is_empty() { None } else { Some(0) }
}

fn read_info(session: &Session, source: String) -> Option<MediaInfo> {
    let props = session.TryGetMediaPropertiesAsync().ok()?.get().ok()?;
    let title = props.Title().ok()?.to_string();
    let artist = props.Artist().ok()?.to_string();
    let playing = session.GetPlaybackInfo().ok()?.PlaybackStatus().ok()? == Status::Playing;

    let timeline = session.GetTimelineProperties().ok()?;
    let start = timeline.StartTime().ok()?.Duration;
    let end = timeline.EndTime().ok()?.Duration;
    let mut pos = timeline.Position().ok()?.Duration;

    // Windows only updates Position when something changes (pause, seek...).
    // While playing we add the time that passed since that update.
    if playing {
        if let Ok(updated) = timeline.LastUpdatedTime() {
            let updated = updated.UniversalTime;
            if updated > 0 {
                let elapsed = (now_ticks() - updated).max(0);
                if elapsed < 432_000_000_000 {
                    pos += elapsed;
                }
            }
        }
    }
    pos = pos.min(end);

    let total = (end - start).max(0) as f32;
    let progress = if total > 0.0 {
        (((pos - start).max(0)) as f32 / total).clamp(0.0, 1.0)
    } else {
        0.0
    };

    Some(MediaInfo { has_media: !title.is_empty(), title, artist, source, playing, progress })
}

/// Checks the media sessions four times a second and updates the UI.
pub fn spawn_poller(ui: Weak<MainWindow>) {
    thread::spawn(move || {
        init_com();
        let mut last = MediaInfo::default();
        let mut last_items: Vec<(String, String, bool)> = Vec::new();
        let mut round: u32 = 0;

        loop {
            let list = list_sessions();
            update_order(&list);
            let active = pick_active(&list);

            let info = match active {
                Some(i) => {
                    let source = friendly(&app_id(&list[i].1));
                    read_info(&list[i].1, source).unwrap_or_default()
                }
                None => MediaInfo::default(),
            };

            if info != last {
                if info.has_media && info.playing && info.title != last.title {
                    crate::ticker::push(&ui, format!("Now playing: {}", info.title));
                }
                let send = info.clone();
                let result = ui.upgrade_in_event_loop(move |ui| {
                    ui.set_has_media(send.has_media);
                    ui.set_playing(send.playing);
                    ui.set_progress(send.progress);
                    ui.set_track_title(send.title.into());
                    ui.set_track_artist(send.artist.into());
                    ui.set_source_name(send.source.into());
                });
                if result.is_err() {
                    break;
                }
                last = info;
            }

            // the list for the source menu is refreshed once a second
            if round % 4 == 0 {
                let selected = SELECTED.lock().ok().and_then(|g| g.clone());
                let mut items: Vec<(String, String, bool)> =
                    vec![(String::new(), "Auto (first opened)".to_string(), selected.is_none())];
                for (key, session) in list.iter().take(4) {
                    let app = friendly(&app_id(session));
                    let title = session
                        .TryGetMediaPropertiesAsync()
                        .ok()
                        .and_then(|op| op.get().ok())
                        .and_then(|p| p.Title().ok())
                        .map(|t| t.to_string())
                        .unwrap_or_default();
                    let label = if title.is_empty() { app } else { format!("{app} - {title}") };
                    let is_selected = selected.as_deref() == Some(key.as_str());
                    items.push((key.clone(), label, is_selected));
                }

                if items != last_items {
                    let send = items.clone();
                    let result = ui.upgrade_in_event_loop(move |ui| {
                        let model: Vec<SourceItem> = send
                            .into_iter()
                            .map(|(key, label, active)| SourceItem {
                                key: SharedString::from(key),
                                label: SharedString::from(label),
                                active,
                            })
                            .collect();
                        ui.set_sources(ModelRc::new(VecModel::from(model)));
                    });
                    if result.is_err() {
                        break;
                    }
                    last_items = items;
                }
            }

            round = round.wrapping_add(1);
            thread::sleep(Duration::from_millis(250));
        }
    });
}

/// Choose which player to control. An empty key means Auto.
pub fn select(key: &str) {
    if let Ok(mut s) = SELECTED.lock() {
        *s = if key.is_empty() { None } else { Some(key.to_string()) };
    }
}

fn with_active(f: impl FnOnce(&Session) + Send + 'static) {
    thread::spawn(move || {
        init_com();
        let list = list_sessions();
        if let Some(i) = pick_active(&list) {
            f(&list[i].1);
        }
    });
}

pub fn toggle() {
    with_active(|s| {
        let _ = s.TryTogglePlayPauseAsync().and_then(|op| op.get());
    });
}

pub fn next() {
    with_active(|s| {
        let _ = s.TrySkipNextAsync().and_then(|op| op.get());
    });
}

pub fn previous() {
    with_active(|s| {
        let _ = s.TrySkipPreviousAsync().and_then(|op| op.get());
    });
}

/// Jump to a position; fraction is 0.0 (start) to 1.0 (end).
pub fn seek(fraction: f32) {
    let f = fraction.clamp(0.0, 1.0) as f64;
    with_active(move |s| {
        let Ok(timeline) = s.GetTimelineProperties() else { return };
        let (Ok(start), Ok(end)) = (timeline.StartTime(), timeline.EndTime()) else { return };
        let target = start.Duration + ((end.Duration - start.Duration) as f64 * f) as i64;
        let _ = s.TryChangePlaybackPositionAsync(target).and_then(|op| op.get());
    });
}