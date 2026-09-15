use chrono::Utc;
use std::{
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

static LOG_PATH: OnceLock<PathBuf> = OnceLock::new();
static WRITE_LOCK: Mutex<()> = Mutex::new(());

pub fn init(data_dir: &Path) {
    let _ = LOG_PATH.set(data_dir.join("gloss.log"));
    install_panic_hook();
    record(format!("app started version={}", env!("CARGO_PKG_VERSION")));
}

pub fn record(message: impl AsRef<str>) {
    let Some(path) = LOG_PATH.get() else {
        return;
    };
    let Ok(_guard) = WRITE_LOCK.try_lock() else {
        return;
    };
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let message = message.as_ref().replace(['\r', '\n'], " ");
    let _ = writeln!(file, "{} {message}", Utc::now().to_rfc3339());
}

fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        record(format!("panic: {info}"));
        previous(info);
    }));
}
