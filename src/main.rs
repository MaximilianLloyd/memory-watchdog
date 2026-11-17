//! memory-watchdog: when macOS memory pressure turns critical, kill the largest
//! runaway dev-tool process (or Chrome tab) so the system does not run out of swap
//! and start killing system helpers (which is what freezes apps).
//!
//! Runs as a launchd agent (local.memory-watchdog). Log: ~/Library/Logs/memory-watchdog.log
//!
//! Env overrides (mainly for testing):
//!   WATCHDOG_PATTERN          regex of process commands that may be killed
//!   WATCHDOG_MIN_MB           minimum memory footprint for a process to be a candidate
//!   WATCHDOG_CHROME_PATTERN   regex of Chrome processes that may be killed
//!   WATCHDOG_CHROME_MIN_MB    minimum footprint for a Chrome candidate
//!   WATCHDOG_FORCE=1          act as if pressure were critical
//!   WATCHDOG_DRY_RUN=1        log the choice but do not kill
//!   WATCHDOG_ONCE=1           run a single check and exit

use regex::Regex;
use std::ffi::CString;
use std::fs::OpenOptions;
use std::io::Write;
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::Duration;

const DEFAULT_PATTERN: &str = r"(^|/| )(tsgolint|tsgo|tsc|tsserver|next-server|esbuild|turbo|vite|vitest|jest|webpack|eslint|node)( |$)|next-server|/node_modules/";
// Renderer processes only, so the browser itself stays open.
const DEFAULT_CHROME_PATTERN: &str = r"Google Chrome.*Helper \(.*Renderer\)";
const INTERVAL: Duration = Duration::from_secs(3);
const SETTLE: Duration = Duration::from_secs(5);
const TERM_GRACE: Duration = Duration::from_secs(3);
const CMD_MAX: usize = 160;

struct Config {
    pattern: Regex,
    min_mb: u64,
    // Lower threshold: Chrome's memory is spread over many tabs rather than one big process.
    chrome_pattern: Regex,
    chrome_min_mb: u64,
    chrome_label: Regex,
    force: bool,
    dry_run: bool,
    once: bool,
    log_path: String,
}

impl Config {
    fn from_env() -> Result<Self, String> {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let flag = |k: &str| var(k).as_deref() == Some("1");
        let num = |k: &str, d: u64| -> Result<u64, String> {
            var(k).map_or(Ok(d), |v| v.parse().map_err(|_| format!("{k} must be an integer, got {v:?}")))
        };
        let re = |k: &str, d: &str| -> Result<Regex, String> {
            let p = var(k).unwrap_or_else(|| d.to_string());
            Regex::new(&p).map_err(|e| format!("{k}: invalid regex: {e}"))
        };
        let home = std::env::var("HOME").map_err(|_| "HOME is not set".to_string())?;
        Ok(Config {
            pattern: re("WATCHDOG_PATTERN", DEFAULT_PATTERN)?,
            min_mb: num("WATCHDOG_MIN_MB", 1500)?,
            chrome_pattern: re("WATCHDOG_CHROME_PATTERN", DEFAULT_CHROME_PATTERN)?,
            chrome_min_mb: num("WATCHDOG_CHROME_MIN_MB", 500)?,
            chrome_label: Regex::new(r"Google Chrome[^/]*Helper \([^)]*\)").unwrap(),
            force: flag("WATCHDOG_FORCE"),
            dry_run: flag("WATCHDOG_DRY_RUN"),
            once: flag("WATCHDOG_ONCE"),
            log_path: format!("{home}/Library/Logs/memory-watchdog.log"),
        })
    }

    fn log(&self, msg: &str) {
        let line = format!("{} {msg}\n", timestamp());
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&self.log_path) {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

fn timestamp() -> String {
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut tm);
        let mut buf = [0u8; 32];
        let fmt = c"%Y-%m-%d %H:%M:%S";
        let n = libc::strftime(buf.as_mut_ptr().cast(), buf.len(), fmt.as_ptr(), &tm);
        String::from_utf8_lossy(&buf[..n]).into_owned()
    }
}

fn sysctl_int(name: &str) -> Option<i64> {
    let cname = CString::new(name).ok()?;
    let mut buf = [0u8; 8];
    let mut len = buf.len();
    let rc = unsafe {
        libc::sysctlbyname(cname.as_ptr(), buf.as_mut_ptr().cast(), &mut len, std::ptr::null_mut(), 0)
    };
    match (rc, len) {
        (0, 4) => Some(i32::from_ne_bytes(buf[..4].try_into().unwrap()) as i64),
        (0, 8) => Some(i64::from_ne_bytes(buf)),
        _ => None,
    }
}

fn is_critical(cfg: &Config) -> bool {
    if cfg.force {
        return true;
    }
    // 1 normal, 2 warn, 4 critical
    let level = sysctl_int("kern.memorystatus_vm_pressure_level");
    // % memory available
    let free = sysctl_int("kern.memorystatus_level").unwrap_or(100);
    matches!(level, Some(4)) || (matches!(level, Some(2)) && free <= 10)
}

fn all_pids() -> Vec<libc::pid_t> {
    unsafe {
        let n = libc::proc_listallpids(std::ptr::null_mut(), 0);
        if n <= 0 {
            return Vec::new();
        }
        // Leave headroom for processes spawned between the two calls.
        let mut pids: Vec<libc::pid_t> = vec![0; n as usize + 64];
        let bytes = (pids.len() * std::mem::size_of::<libc::pid_t>()) as libc::c_int;
        let n = libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes);
        pids.truncate(n.max(0) as usize);
        pids.retain(|&p| p > 0);
        pids
    }
}

fn proc_uid(pid: libc::pid_t) -> Option<u32> {
    unsafe {
        let mut info: libc::proc_bsdinfo = std::mem::zeroed();
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let n = libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&mut info as *mut libc::proc_bsdinfo).cast(), size);
        (n == size).then_some(info.pbi_uid)
    }
}

/// Physical footprint in MB: the same number top and Activity Monitor show as
/// "Memory" (includes compressed memory, unlike RSS).
fn footprint_mb(pid: libc::pid_t) -> Option<u64> {
    unsafe {
        let mut ri: libc::rusage_info_v2 = std::mem::zeroed();
        let rc = libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V2, (&mut ri as *mut libc::rusage_info_v2).cast());
        (rc == 0).then_some(ri.ri_phys_footprint / (1024 * 1024))
    }
}

/// Full command line (argv joined by spaces), like `ps -o command=`.
fn command_line(pid: libc::pid_t) -> Option<String> {
    unsafe {
        let mut argmax: libc::c_int = 0;
        let mut len = std::mem::size_of::<libc::c_int>();
        let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
        if libc::sysctl(mib.as_mut_ptr(), 2, (&mut argmax as *mut libc::c_int).cast(), &mut len, std::ptr::null_mut(), 0) != 0 {
            return None;
        }
        let mut buf = vec![0u8; argmax as usize];
        let mut len = buf.len();
        let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
        if libc::sysctl(mib.as_mut_ptr(), 3, buf.as_mut_ptr().cast(), &mut len, std::ptr::null_mut(), 0) != 0 {
            return None;
        }
        buf.truncate(len);
        parse_procargs2(&buf)
    }
}

/// KERN_PROCARGS2 layout: argc (i32), exec path, NUL padding, then argc NUL-terminated args.
fn parse_procargs2(buf: &[u8]) -> Option<String> {
    let argc = i32::from_ne_bytes(buf.get(..4)?.try_into().ok()?) as usize;
    let rest = &buf[4..];
    let mut i = rest.iter().position(|&b| b == 0)?;
    while rest.get(i) == Some(&0) {
        i += 1;
    }
    let args: Vec<String> = rest[i..]
        .split(|&b| b == 0)
        .take(argc)
        .map(|a| String::from_utf8_lossy(a).into_owned())
        .collect();
    (!args.is_empty()).then(|| args.join(" "))
}

fn truncate(s: &str, max: usize) -> &str {
    match s.char_indices().nth(max) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

struct Victim {
    pid: libc::pid_t,
    mb: u64,
    label: String,
}

/// The largest process owned by this user that matches a kill pattern and is over its threshold.
fn pick_victim(cfg: &Config) -> Option<Victim> {
    let uid = unsafe { libc::getuid() };
    let me = std::process::id() as libc::pid_t;
    let floor = cfg.min_mb.min(cfg.chrome_min_mb);
    let mut procs: Vec<(libc::pid_t, u64)> = all_pids()
        .into_iter()
        .filter(|&p| p != me)
        .filter_map(|p| footprint_mb(p).filter(|&mb| mb >= floor).map(|mb| (p, mb)))
        .collect();
    procs.sort_by(|a, b| b.1.cmp(&a.1));

    for (pid, mb) in procs {
        if proc_uid(pid) != Some(uid) {
            continue;
        }
        let Some(cmd) = command_line(pid) else { continue };
        if mb >= cfg.min_mb && cfg.pattern.is_match(&cmd) {
            return Some(Victim { pid, mb, label: truncate(&cmd, CMD_MAX).to_string() });
        }
        if mb >= cfg.chrome_min_mb && cfg.chrome_pattern.is_match(&cmd) {
            let tab = cfg.chrome_label.find(&cmd).map_or("Chrome renderer", |m| m.as_str());
            return Some(Victim { pid, mb, label: format!("Chrome tab: {tab}") });
        }
    }
    None
}

fn alive(pid: libc::pid_t) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}

fn notify(label: &str, mb: u64) {
    let text = truncate(label, 60).replace(['\\', '"'], "");
    let script = format!("display notification \"Killed {text} ({mb} MB)\" with title \"Memory watchdog\"");
    let _ = Command::new("osascript")
        .args(["-e", &script])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

fn act(cfg: &Config) -> bool {
    let Some(v) = pick_victim(cfg) else {
        cfg.log(&format!("critical pressure, but no dev process over {}MB to kill", cfg.min_mb));
        return false;
    };
    if cfg.dry_run {
        cfg.log(&format!("DRY RUN would kill pid={} {}MB: {}", v.pid, v.mb, v.label));
        return true;
    }
    cfg.log(&format!("critical pressure: killing pid={} {}MB: {}", v.pid, v.mb, v.label));
    unsafe { libc::kill(v.pid, libc::SIGTERM) };
    sleep(TERM_GRACE);
    if alive(v.pid) {
        unsafe { libc::kill(v.pid, libc::SIGKILL) };
    }
    notify(&v.label, v.mb);
    true
}

fn main() {
    let cfg = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("memory-watchdog: {e}");
            std::process::exit(2);
        }
    };

    if cfg.once {
        if is_critical(&cfg) {
            act(&cfg);
        }
        return;
    }

    cfg.log(&format!("started (min {}MB, interval {}s)", cfg.min_mb, INTERVAL.as_secs()));
    loop {
        if is_critical(&cfg) {
            act(&cfg);
            sleep(SETTLE); // let memory settle before deciding to kill another
        } else {
            sleep(INTERVAL);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_procargs2() {
        let mut buf = 3i32.to_ne_bytes().to_vec();
        buf.extend_from_slice(b"/usr/bin/node\0\0\0\0node\0server.js\0--port=3\0FOO=bar\0");
        assert_eq!(parse_procargs2(&buf).as_deref(), Some("node server.js --port=3"));
    }

    #[test]
    fn default_pattern_matches_dev_tools_only() {
        let re = Regex::new(DEFAULT_PATTERN).unwrap();
        assert!(re.is_match("/Users/x/.nvm/versions/node/v24/bin/node /x/tsserver.js"));
        assert!(re.is_match("/repo/node_modules/.pnpm/@typescript+native/tsgo --lsp"));
        assert!(re.is_match("next-server (v15)"));
        assert!(!re.is_match("/Applications/Slack.app/Contents/MacOS/Slack"));
    }

    #[test]
    fn chrome_pattern_matches_renderers_only() {
        let re = Regex::new(DEFAULT_CHROME_PATTERN).unwrap();
        assert!(re.is_match("/Applications/Google Chrome.app/.../Google Chrome Helper (Renderer).app/Contents/MacOS/Google Chrome Helper (Renderer) --type=renderer"));
        assert!(!re.is_match("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"));
        assert!(!re.is_match("/Applications/Google Chrome.app/.../Google Chrome Helper (GPU).app/Contents/MacOS/Google Chrome Helper (GPU)"));
    }

    #[test]
    fn truncates_on_char_boundary() {
        assert_eq!(truncate("åäö", 2), "åä");
        assert_eq!(truncate("ab", 5), "ab");
    }
}
