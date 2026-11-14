use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use arboard::Clipboard;
use slint::{ModelRc, SharedString, VecModel, Weak};

use crate::app_state::CLIPBOARD_MAX;
use crate::MainWindow;

static HISTORY: OnceLock<Mutex<Vec<String>>> = OnceLock::new();

fn history() -> &'static Mutex<Vec<String>> {
    HISTORY.get_or_init(|| Mutex::new(Vec::new()))
}

/// One-line preview of a clip for the list.
fn label(text: &str) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    one_line.chars().take(80).collect()
}

/// Put the clip at `index` back on the clipboard.
pub fn copy(index: usize) -> bool {
    let text = match history().lock() {
        Ok(h) => h.get(index).cloned(),
        Err(_) => None,
    };
    let Some(text) = text else { return false };
    Clipboard::new().and_then(|mut cb| cb.set_text(text)).is_ok()
}

/// Watches the clipboard and keeps the last few text entries (memory only, nothing is saved to disk).
pub fn spawn_watcher(ui: Weak<MainWindow>) {
    thread::spawn(move || loop {
        if let Ok(mut cb) = Clipboard::new() {
            if let Ok(text) = cb.get_text() {
                let t = text.trim().to_string();
                if !t.is_empty() {
                    let mut snapshot: Option<Vec<String>> = None;
                    if let Ok(mut h) = history().lock() {
                        let is_new = h.first().map(|first| first != &t).unwrap_or(true);
                        if is_new {
                            h.retain(|x| x != &t);
                            h.insert(0, t);
                            h.truncate(CLIPBOARD_MAX);
                            snapshot = Some(h.iter().map(|s| label(s)).collect());
                        }
                    }
                    if let Some(items) = snapshot {
                        let result = ui.upgrade_in_event_loop(move |ui| {
                            let model: Vec<SharedString> =
                                items.into_iter().map(SharedString::from).collect();
                            ui.set_clips(ModelRc::new(VecModel::from(model)));
                        });
                        if result.is_err() {
                            return;
                        }
                    }
                }
            }
        }
        thread::sleep(Duration::from_millis(700));
    });
}