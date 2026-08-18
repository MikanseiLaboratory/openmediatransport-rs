//! Process file logging compatible with libomtnet `OMTLogging`.
//!
//! Log files are `{storage}/logs/{exe}{pid}.log`. The storage directory is the
//! same as [`crate::Settings`] (`settings.xml`):
//!
//! - Windows: `%ProgramData%\OMT\logs` (`C:\ProgramData\OMT\logs`)
//! - macOS / Linux: `~/.OMT/logs`
//! - Override the storage directory with [`crate::OMT_STORAGE_PATH`]
//!
//! [`init_logging`] is called automatically from sender, receiver, and discovery
//! constructors. Applications may also call it at startup.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

static LOG_FILE: OnceLock<Mutex<Option<File>>> = OnceLock::new();

/// `{storage}/logs`.
pub fn logs_dir() -> PathBuf {
    crate::settings::storage_dir().join("logs")
}

/// `{exe_name}{pid}.log` under [`logs_dir`], matching libomtnet `OMTLogging`.
pub fn default_log_path() -> PathBuf {
    log_path_in(&logs_dir())
}

fn log_path_in(dir: &Path) -> PathBuf {
    dir.join(format!("{}.log", process_name_and_id()))
}

fn process_name_and_id() -> String {
    let pid = std::process::id();
    match std::env::current_exe() {
        Ok(exe) => match exe.file_name() {
            Some(name) => format!("{}{pid}", name.to_string_lossy()),
            None => pid.to_string(),
        },
        Err(_) => pid.to_string(),
    }
}

fn open_append(path: &Path) -> io::Result<File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    OpenOptions::new().create(true).append(true).open(path)
}

fn ensure_file() -> &'static Mutex<Option<File>> {
    LOG_FILE.get_or_init(|| match open_append(&default_log_path()) {
        Ok(mut file) => {
            let _ = writeln!(file, "{},[OMTLogging],Log Started", timestamp());
            let _ = file.flush();
            Mutex::new(Some(file))
        }
        Err(_) => Mutex::new(None),
    })
}

/// Open the process log file (best-effort). Safe to call more than once.
pub fn init_logging() {
    let _ = ensure_file();
    tracing::trace!("openmediatransport logging ready");
}

/// Append a libomtnet-style line: `{datetime},[{source}],{message}`.
pub fn write(message: &str, source: &str) {
    let line = format!("{},[{}],{message}", timestamp(), source);
    let Ok(mut guard) = ensure_file().lock() else {
        return;
    };
    if let Some(file) = guard.as_mut() {
        let _ = writeln!(file, "{line}");
        let _ = file.flush();
    }
}

/// Log a debug message to tracing and the OMT log file.
pub fn debug(msg: &str) {
    write(msg, "openmediatransport");
    tracing::debug!("{msg}");
}

/// Log an info message to tracing and the OMT log file.
pub fn info(msg: &str) {
    write(msg, "openmediatransport");
    tracing::info!("{msg}");
}

/// Log a warning to tracing and the OMT log file.
pub fn warn(msg: &str) {
    write(msg, "openmediatransport");
    tracing::warn!("{msg}");
}

/// Log an error to tracing and the OMT log file.
pub fn error(msg: &str) {
    write(msg, "openmediatransport");
    tracing::error!("{msg}");
}

fn timestamp() -> String {
    #[cfg(windows)]
    {
        #[repr(C)]
        struct SystemTime {
            w_year: u16,
            w_month: u16,
            w_day_of_week: u16,
            w_day: u16,
            w_hour: u16,
            w_minute: u16,
            w_second: u16,
            w_milliseconds: u16,
        }
        unsafe extern "system" {
            fn GetLocalTime(lp_system_time: *mut SystemTime);
        }
        let mut st = SystemTime {
            w_year: 0,
            w_month: 0,
            w_day_of_week: 0,
            w_day: 0,
            w_hour: 0,
            w_minute: 0,
            w_second: 0,
            w_milliseconds: 0,
        };
        unsafe { GetLocalTime(&mut st) };
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            st.w_year, st.w_month, st.w_day, st.w_hour, st.w_minute, st.w_second
        )
    }
    #[cfg(unix)]
    {
        #[repr(C)]
        struct Tm {
            tm_sec: i32,
            tm_min: i32,
            tm_hour: i32,
            tm_mday: i32,
            tm_mon: i32,
            tm_year: i32,
            tm_wday: i32,
            tm_yday: i32,
            tm_isdst: i32,
            _pad: [u8; 32],
        }
        unsafe extern "C" {
            fn time(tloc: *mut i64) -> i64;
            fn localtime_r(timep: *const i64, result: *mut Tm) -> *mut Tm;
        }
        unsafe {
            let mut t: i64 = 0;
            time(&mut t);
            let mut tm = Tm {
                tm_sec: 0,
                tm_min: 0,
                tm_hour: 0,
                tm_mday: 0,
                tm_mon: 0,
                tm_year: 0,
                tm_wday: 0,
                tm_yday: 0,
                tm_isdst: 0,
                _pad: [0; 32],
            };
            if localtime_r(&t, &mut tm).is_null() {
                return format!("{t}");
            }
            format!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                tm.tm_year + 1900,
                tm.tm_mon + 1,
                tm.tm_mday,
                tm.tm_hour,
                tm.tm_min,
                tm.tm_sec
            )
        }
    }
    #[cfg(not(any(windows, unix)))]
    {
        String::from("unknown-time")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logs_dir_is_under_settings_storage() {
        let dir = logs_dir();
        assert!(
            dir.ends_with("logs"),
            "logs dir should end with logs: {}",
            dir.display()
        );
    }

    #[test]
    fn log_filename_is_module_name_plus_pid() {
        let path = log_path_in(Path::new("/tmp/logs"));
        let name = path.file_name().unwrap().to_string_lossy();
        assert!(
            name.ends_with(&format!("{}.log", std::process::id())),
            "expected libomtnet ModuleName+pid.log, got {name}"
        );
    }
}
