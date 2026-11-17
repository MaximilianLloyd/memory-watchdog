//! memory-watchdog: when macOS memory pressure turns critical, kill the largest
//! runaway process that matches a configured rule, so the system does not run out
//! of swap and start killing system helpers (which is what freezes apps).
//!
//! Usage:
//!   memory-watchdog [--config PATH] [--once] [--dry-run] [--force]
//!   memory-watchdog check [--config PATH]
//!
//! Rules live in ~/.config/memory-watchdog/config.toml (see config.toml in the repo).

use regex::Regex;
use serde::Deserialize;
use std::ffi::CString;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime};

const CMD_MAX: usize = 160;
const NOTIFIER: &str = "Applications/Memory Watchdog.app/Contents/MacOS/memory-watchdog-notify";

// ---------- configuration ----------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    #[serde(default = "d_interval")]
    interval_secs: u64,
    #[serde(default = "d_settle")]
    settle_secs: u64,
    #[serde(default = "d_grace")]
    grace_secs: u64,
    #[serde(default = "d_warn_free")]
    warn_free_percent: i64,
    #[serde(default = "d_true")]
    notify: bool,
    #[serde(default, rename = "rule")]
    rules: Vec<RuleConfig>,
}
fn d_interval() -> u64 { 3 }
fn d_settle() -> u64 { 5 }
fn d_grace() -> u64 { 3 }
fn d_warn_free() -> i64 { 10 }
fn d_true() -> bool { true }

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleConfig {
    name: String,
    min_mb: u64,
    #[serde(default)]
    processes: Vec<String>,
    #[serde(default)]
    patterns: Vec<String>,
    label: Option<String>,
}

struct Rule {
    name: String,
    min_mb: u64,
    re: Regex,
    label: Option<String>,
}

impl Rule {
    fn compile(rc: RuleConfig) -> Result<Rule, String> {
        if rc.processes.is_empty() && rc.patterns.is_empty() {
            return Err(format!("rule {:?} needs at least one entry in processes or patterns", rc.name));
        }
        let mut alts = Vec::new();
        if !rc.processes.is_empty() {
            let names: Vec<String> = rc.processes.iter().map(|n| regex::escape(n)).collect();
            // Whole word in the command line, or the last component of a path.
            alts.push(format!("(?:^|/| )(?:{})(?: |$)", names.join("|")));
        }
        for p in &rc.patterns {
            Regex::new(p).map_err(|e| format!("rule {:?}: invalid pattern {p:?}: {e}", rc.name))?;
            alts.push(format!("(?:{p})"));
        }
        let re = Regex::new(&alts.join("|")).map_err(|e| format!("rule {:?}: {e}", rc.name))?;
        Ok(Rule { name: rc.name, min_mb: rc.min_mb, re, label: rc.label })
    }
}

struct Config {
    interval: Duration,
    settle: Duration,
    grace: Duration,
    warn_free_percent: i64,
    notify: bool,
    rules: Vec<Rule>,
}

impl Config {
    fn parse(text: &str) -> Result<Config, String> {
        let fc: FileConfig = toml::from_str(text).map_err(|e| e.to_string())?;
        if fc.rules.is_empty() {
            return Err("no [[rule]] entries: nothing would ever be killed".into());
        }
        let rules = fc.rules.into_iter().map(Rule::compile).collect::<Result<Vec<_>, _>>()?;
        Ok(Config {
            interval: Duration::from_secs(fc.interval_secs.max(1)),
            settle: Duration::from_secs(fc.settle_secs),
            grace: Duration::from_secs(fc.grace_secs),
            warn_free_percent: fc.warn_free_percent,
            notify: fc.notify,
            rules,
        })
    }

    fn load(path: &Path) -> Result<Config, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Config::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }
}

struct Opts {
    config_path: PathBuf,
    check: bool,
    once: bool,
    dry_run: bool,
    force: bool,
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into()))
}

fn parse_args() -> Result<Opts, String> {
    let env1 = |k: &str| std::env::var(k).as_deref() == Ok("1");
    let default_config = std::env::var("WATCHDOG_CONFIG").map(PathBuf::from).unwrap_or_else(|_| {
        let base = std::env::var("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|_| home().join(".config"));
        base.join("memory-watchdog/config.toml")
    });
    let mut o = Opts {
        config_path: default_config,
        check: false,
        once: env1("WATCHDOG_ONCE"),
        dry_run: env1("WATCHDOG_DRY_RUN"),
        force: env1("WATCHDOG_FORCE"),
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "check" => o.check = true,
            "--once" => o.once = true,
            "--dry-run" => o.dry_run = true,
            "--force" => o.force = true,
            "--config" => o.config_path = args.next().ok_or("--config needs a path")?.into(),
            "-h" | "--help" => {
                println!("usage: memory-watchdog [--config PATH] [--once] [--dry-run] [--force]\n       memory-watchdog check [--config PATH]");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other:?} (try --help)")),
        }
    }
    Ok(o)
}

// ---------- logging ----------

fn log_path() -> PathBuf {
    home().join("Library/Logs/memory-watchdog.log")
}

fn log(msg: &str) {
    let line = format!("{} {msg}\n", timestamp());
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(log_path()) {
        let _ = f.write_all(line.as_bytes());
    }
}

fn timestamp() -> String {
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut tm);
        let mut buf = [0u8; 32];
        let n = libc::strftime(buf.as_mut_ptr().cast(), buf.len(), c"%Y-%m-%d %H:%M:%S".as_ptr(), &tm);
        String::from_utf8_lossy(&buf[..n]).into_owned()
    }
}

// ---------- macOS process and memory APIs ----------

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

struct Pressure {
    level: Option<i64>, // 1 normal, 2 warn, 4 critical
    free: i64,          // % memory available
}

fn pressure() -> Pressure {
    Pressure {
        level: sysctl_int("kern.memorystatus_vm_pressure_level"),
        free: sysctl_int("kern.memorystatus_level").unwrap_or(100),
    }
}

impl Pressure {
    fn name(&self) -> &'static str {
        match self.level {
            Some(1) => "normal",
            Some(2) => "warn",
            Some(4) => "critical",
            _ => "unknown",
        }
    }
    fn critical(&self, cfg: &Config) -> bool {
        self.level == Some(4) || (self.level == Some(2) && self.free <= cfg.warn_free_percent)
    }
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

/// Physical footprint in MB: the number top and Activity Monitor show as
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

// ---------- choosing and killing ----------

struct Victim {
    pid: libc::pid_t,
    mb: u64,
    rule: String,
    label: String,
}

/// Which rule (if any) allows killing this process, and how to describe it.
fn match_rule<'a>(rules: &'a [Rule], cmd: &str, mb: u64) -> Option<(&'a Rule, String)> {
    rules.iter().filter(|r| mb >= r.min_mb).find_map(|r| {
        let m = r.re.find(cmd)?;
        let label = match &r.label {
            Some(l) => format!("{l}: {}", m.as_str().trim()),
            None => truncate(cmd, CMD_MAX).to_string(),
        };
        Some((r, label))
    })
}

/// The largest process owned by this user that some rule allows killing.
fn pick_victim(cfg: &Config) -> Option<Victim> {
    let uid = unsafe { libc::getuid() };
    let me = std::process::id() as libc::pid_t;
    let floor = cfg.rules.iter().map(|r| r.min_mb).min()?;
    let mut procs: Vec<(libc::pid_t, u64)> = all_pids()
        .into_iter()
        .filter(|&p| p != me)
        .filter_map(|p| footprint_mb(p).filter(|&mb| mb >= floor).map(|mb| (p, mb)))
        .collect();
    procs.sort_by(|a, b| b.1.cmp(&a.1));

    procs.into_iter().find_map(|(pid, mb)| {
        if proc_uid(pid) != Some(uid) {
            return None;
        }
        let cmd = command_line(pid)?;
        let (rule, label) = match_rule(&cfg.rules, &cmd, mb)?;
        Some(Victim { pid, mb, rule: rule.name.clone(), label })
    })
}

fn alive(pid: libc::pid_t) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}

/// Post a notification from the Memory Watchdog app (so it carries the app icon).
/// Falls back to osascript when the app is missing or not allowed to notify.
/// Runs on its own thread so a slow notification never delays the watchdog.
fn notify(title: String, body: String) {
    std::thread::spawn(move || {
        if !notify_with_app(&title, &body) {
            let esc = |s: &str| s.replace(['\\', '"'], "");
            let script = format!("display notification \"{}\" with title \"{}\"", esc(&body), esc(&title));
            let _ = Command::new("osascript").args(["-e", &script]).stdout(Stdio::null()).stderr(Stdio::null()).status();
        }
    });
}

fn notify_with_app(title: &str, body: &str) -> bool {
    let helper = home().join(NOTIFIER);
    let Ok(mut child) = Command::new(&helper)
        .args([title, body])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() < deadline => sleep(Duration::from_millis(100)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

fn act(cfg: &Config, dry_run: bool) {
    let Some(v) = pick_victim(cfg) else {
        log("critical pressure, but no process matches a rule");
        return;
    };
    if dry_run {
        log(&format!("DRY RUN would kill pid={} {}MB [{}]: {}", v.pid, v.mb, v.rule, v.label));
        return;
    }
    log(&format!("critical pressure: killing pid={} {}MB [{}]: {}", v.pid, v.mb, v.rule, v.label));
    unsafe { libc::kill(v.pid, libc::SIGTERM) };
    sleep(cfg.grace);
    if alive(v.pid) {
        unsafe { libc::kill(v.pid, libc::SIGKILL) };
    }
    if cfg.notify {
        notify(format!("Freed {} MB", v.mb), format!("Killed {}", truncate(&v.label, 90)));
    }
}

// ---------- entry points ----------

/// `memory-watchdog check`: validate the config and show what would happen right now.
fn check(path: &Path) -> i32 {
    let cfg = match Config::load(path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("config error: {e}");
            return 1;
        }
    };
    let p = pressure();
    println!("config:   {}", path.display());
    println!("pressure: {} ({}% memory available, acts at critical or warn with <= {}%)",
             p.name(), p.free, cfg.warn_free_percent);
    for r in &cfg.rules {
        println!("rule:     {} (min {} MB{})", r.name, r.min_mb,
                 r.label.as_deref().map(|l| format!(", label {l:?}")).unwrap_or_default());
    }
    match pick_victim(&cfg) {
        Some(v) => println!("would kill: pid {} {} MB [{}] {}", v.pid, v.mb, v.rule, v.label),
        None => println!("would kill: nothing (no matching process over its threshold)"),
    }
    0
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn main() {
    let opts = match parse_args() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("memory-watchdog: {e}");
            std::process::exit(2);
        }
    };
    if opts.check {
        std::process::exit(check(&opts.config_path));
    }

    let mut cfg = match Config::load(&opts.config_path) {
        Ok(c) => c,
        Err(e) => {
            log(&format!("config error, not starting: {e}"));
            eprintln!("memory-watchdog: {e}");
            std::process::exit(1);
        }
    };
    let critical = |cfg: &Config| opts.force || pressure().critical(cfg);

    if opts.once {
        if critical(&cfg) {
            act(&cfg, opts.dry_run);
        }
        return;
    }

    let mut loaded_at = mtime(&opts.config_path);
    log(&format!("started ({} rules, interval {}s, config {})",
                 cfg.rules.len(), cfg.interval.as_secs(), opts.config_path.display()));
    loop {
        // Pick up config edits without a restart; keep the old config if the new one is broken.
        let now = mtime(&opts.config_path);
        if now != loaded_at {
            loaded_at = now;
            match Config::load(&opts.config_path) {
                Ok(c) => {
                    cfg = c;
                    log(&format!("config reloaded ({} rules)", cfg.rules.len()));
                }
                Err(e) => log(&format!("config error, keeping previous config: {e}")),
            }
        }
        if critical(&cfg) {
            act(&cfg, opts.dry_run);
            sleep(cfg.settle); // let memory settle before deciding to kill another
        } else {
            sleep(cfg.interval);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shipped() -> Config {
        Config::parse(include_str!("../config.toml")).expect("shipped config.toml parses")
    }

    #[test]
    fn parses_procargs2() {
        let mut buf = 3i32.to_ne_bytes().to_vec();
        buf.extend_from_slice(b"/usr/bin/node\0\0\0\0node\0server.js\0--port=3\0FOO=bar\0");
        assert_eq!(parse_procargs2(&buf).as_deref(), Some("node server.js --port=3"));
    }

    #[test]
    fn dev_tools_rule_matches_dev_tools_only() {
        let cfg = shipped();
        let m = |cmd: &str| match_rule(&cfg.rules, cmd, 2000).map(|(r, _)| r.name.as_str());
        assert_eq!(m("/Users/x/.nvm/versions/node/v24/bin/node /x/tsserver.js"), Some("dev tools"));
        assert_eq!(m("/repo/node_modules/.pnpm/@typescript+native/tsgo --lsp"), Some("dev tools"));
        assert_eq!(m("next-server (v15)"), Some("dev tools"));
        assert_eq!(m("/usr/local/bin/nodemon app.js"), None);
        assert_eq!(m("/Applications/Slack.app/Contents/MacOS/Slack"), None);
    }

    #[test]
    fn thresholds_apply_per_rule() {
        let cfg = shipped();
        assert!(match_rule(&cfg.rules, "/usr/bin/node server.js", 1499).is_none());
        let tab = "/Applications/Google Chrome.app/x/Google Chrome Helper (Renderer).app/Contents/MacOS/Google Chrome Helper (Renderer) --type=renderer";
        assert!(match_rule(&cfg.rules, tab, 499).is_none());
        let (r, label) = match_rule(&cfg.rules, tab, 600).unwrap();
        assert_eq!(r.name, "chrome tabs");
        assert_eq!(label, "Chrome tab: Google Chrome Helper (Renderer)");
    }

    #[test]
    fn chrome_rule_spares_browser_and_gpu() {
        let cfg = shipped();
        assert!(match_rule(&cfg.rules, "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", 9000).is_none());
        assert!(match_rule(&cfg.rules, "/Applications/Google Chrome.app/x/Google Chrome Helper (GPU).app/Contents/MacOS/Google Chrome Helper (GPU)", 9000).is_none());
    }

    #[test]
    fn rejects_bad_configs() {
        assert!(Config::parse("").is_err());
        assert!(Config::parse("[[rule]]\nname = \"x\"\nmin_mb = 1").is_err());
        assert!(Config::parse("[[rule]]\nname = \"x\"\nmin_mb = 1\npatterns = [\"(\"]").is_err());
        assert!(Config::parse("typo = 1\n[[rule]]\nname = \"x\"\nmin_mb = 1\nprocesses = [\"a\"]").is_err());
    }

    #[test]
    fn truncates_on_char_boundary() {
        assert_eq!(truncate("åäö", 2), "åä");
        assert_eq!(truncate("ab", 5), "ab");
    }
}
