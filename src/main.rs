use std::collections::VecDeque;
use std::fs;
use std::io::{BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::process;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

const RUNTIME_DIR: &str = "/run/batteryremainingd";
const SOCKET_PATH: &str = "/run/batteryremainingd/batteryremainingd.sock";
const BATTERY_BASE: &str = "/sys/class/power_supply";
const POLL_INTERVAL: Duration = Duration::from_secs(1);
const BATTERY_RESCAN_INTERVAL: Duration = Duration::from_secs(10);
const TREND_SAMPLES: usize = 60;
const MAX_REQUEST_SIZE: usize = 4096;

#[derive(Clone)]
#[allow(dead_code)]
struct BatterySample {
    power: f64,
    energy_remaining: f64,
    status: String,
    capacity: f64,
}

struct TrendData {
    samples: VecDeque<BatterySample>,
}

impl TrendData {
    fn new() -> Self {
        TrendData { samples: VecDeque::with_capacity(TREND_SAMPLES) }
    }

    fn push(&mut self, s: BatterySample) {
        self.samples.push_back(s);
        while self.samples.len() > TREND_SAMPLES {
            self.samples.pop_front();
        }
    }

    fn latest(&self) -> Option<&BatterySample> {
        self.samples.back()
    }

    fn smoothed_power(&self) -> Option<f64> {
        let n = self.samples.len();
        if n == 0 {
            return None;
        }
        let total: f64 = (1..=n).map(|i| i as f64).sum();
        let weighted: f64 = self.samples.iter().enumerate()
            .map(|(i, s)| s.power * (i as f64 + 1.0))
            .sum();
        Some(weighted / total)
    }

    fn power_trend_slope(&self) -> f64 {
        let n = self.samples.len();
        if n < 3 {
            return 0.0;
        }
        let mean_x = (n - 1) as f64 / 2.0;
        let mean_y: f64 = self.samples.iter().map(|s| s.power).sum::<f64>() / n as f64;
        let mut num = 0.0;
        let mut den = 0.0;
        for (i, s) in self.samples.iter().enumerate() {
            let x = i as f64;
            let y = s.power;
            num += (x - mean_x) * (y - mean_y);
            den += (x - mean_x) * (x - mean_x);
        }
        if den == 0.0 { return 0.0; }
        let slope = num / den;
        if mean_y == 0.0 { return 0.0; }
        slope / mean_y
    }
}

struct ParsedArgs {
    help: bool,
    daemon: bool,
    monitor: bool,
    format: Option<String>,
    include_charging: bool,
}

fn parse_args() -> Result<ParsedArgs, String> {
    let args: Vec<String> = std::env::args().collect();
    let mut parsed = ParsedArgs {
        help: false,
        daemon: false,
        monitor: false,
        format: None,
        include_charging: false,
    };

    if args.len() == 1 {
        parsed.help = true;
        return Ok(parsed);
    }

    let mut i = 1;
    while i < args.len() {
        let arg = &args[i];

        if arg == "--" {
            break;
        }

        if arg.starts_with("--") {
            match arg.as_str() {
                "--help" => parsed.help = true,
                "--daemon" => parsed.daemon = true,
                "--monitor" => parsed.monitor = true,
                "--format" => {
                    i += 1;
                    if i >= args.len() {
                        return Err("error: --format requires a format string".to_string());
                    }
                    parsed.format = Some(args[i].clone());
                }
                "--include-charging" => parsed.include_charging = true,
                _ => return Err(format!("error: unknown flag '{}'", arg)),
            }
        } else if arg.starts_with("-") && arg.len() > 1 {
            if arg == "-f" {
                i += 1;
                if i >= args.len() {
                    return Err("error: -f requires a format string".to_string());
                }
                parsed.format = Some(args[i].clone());
            } else if let Some(rest) = arg.strip_prefix("-f") {
                parsed.format = Some(rest.to_string());
            } else {
                for c in arg[1..].chars() {
                    match c {
                        'h' => parsed.help = true,
                        'd' => parsed.daemon = true,
                        'm' => parsed.monitor = true,
                        'c' => parsed.include_charging = true,
                        '0' => {}
                        'f' => return Err("error: -f cannot be combined with other flags".to_string()),
                        _ => return Err(format!("error: unknown flag '-{}'", c)),
                    }
                }
            }
        } else {
            return Err(format!("error: unexpected argument '{}'", arg));
        }

        i += 1;
    }

    Ok(parsed)
}

fn main() {
    let parsed = match parse_args() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{}", e);
            process::exit(1);
        }
    };

    if parsed.help {
        print_help();
        return;
    }

    if parsed.daemon {
        match run_daemon() {
            Ok(()) => process::exit(0),
            Err(e) => {
                eprintln!("fatal: {}", e);
                process::exit(1);
            }
        }
    }

    if parsed.monitor {
        run_monitor(
            parsed.format.as_deref().unwrap_or("[STATUS] %p | [TIME] %H:%M"),
            parsed.include_charging,
        );
        return;
    }

    let format = parsed.format.as_deref().unwrap_or("%m");

    match query_socket(format, parsed.include_charging) {
        Ok(resp) => println!("{}", resp),
        Err(e) => {
            eprintln!("error: {}", e);
            process::exit(1);
        }
    }
}

fn print_help() {
    eprintln!("Usage: batteryremainingd [OPTIONS]");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  -d, --daemon              Start the daemon");
    eprintln!("  -m, --monitor             Continuously monitor battery status (default format: \"[STATUS] %p | [TIME] %H:%M\")");
    eprintln!("  -f, --format FORMAT       Query daemon with format string");
    eprintln!("  -c, --include-charging    Show time-until-full when charging");
    eprintln!("  -0                        No-op flag (terminator for combined groups)");
    eprintln!("  -h, --help                Show this help");
    eprintln!();
    eprintln!("Short flags can be combined: -mc, -cm, -mc0");
    eprintln!();
    eprintln!("Format specifiers:");
    eprintln!("  %p    power status (Charging / Draining / Full)");
    eprintln!("  %m    total minutes (default)");
    eprintln!("  %h    total hours (float)");
    eprintln!("  %d    days component, zero-padded");
    eprintln!("  %D    total days (float)");
    eprintln!("  %H    hours component, zero-padded");
    eprintln!("  %M    minutes component, zero-padded");
    eprintln!("  %S    seconds component (always 00)");
    eprintln!("  %s    total seconds (always 0)");
    eprintln!("  %c    battery capacity percentage (0-100)");
    eprintln!("  %C    battery capacity percentage, zero-padded (3 digits)");
    eprintln!("  %%    literal %%");
    eprintln!();
    eprintln!("Examples:");
    eprintln!("  batteryremainingd -d");
    eprintln!("  batteryremainingd -m");
    eprintln!("  batteryremainingd -m -c");
    eprintln!("  batteryremainingd -m -f '[STATUS] %p | [TIME] %Hh %Mm %Ss'");
    eprintln!("  batteryremainingd -f '%m'");
    eprintln!("  batteryremainingd -c -f '%Hh %Mm'");
    eprintln!("  batteryremainingd -f '%dd %Hh %Mm'");
}

fn find_battery() -> Option<String> {
    let entries = fs::read_dir(BATTERY_BASE).ok()?;
    for entry in entries {
        let entry = entry.ok()?;
        let p = entry.path();
        if !p.is_dir() { continue; }
        if let Ok(t) = fs::read_to_string(p.join("type")) {
            if t.trim() == "Battery" {
                return p.to_str().map(|s| s.to_string());
            }
        }
    }
    None
}

fn read_u64(path: &str) -> Option<u64> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn read_string(path: &str) -> Option<String> {
    Some(fs::read_to_string(path).ok()?.trim().to_string())
}

fn read_battery(bat_path: &str) -> Option<BatterySample> {
    let energy = read_u64(&format!("{}/energy_now", bat_path))
        .or_else(|| {
            let charge = read_u64(&format!("{}/charge_now", bat_path))?;
            let voltage = read_u64(&format!("{}/voltage_now", bat_path))?;
            Some(charge * voltage / 1_000_000)
        })?;

    let energy_full = read_u64(&format!("{}/energy_full", bat_path))
        .or_else(|| {
            let charge_full = read_u64(&format!("{}/charge_full", bat_path))?;
            let voltage = read_u64(&format!("{}/voltage_now", bat_path))?;
            Some(charge_full * voltage / 1_000_000)
        })?;

    let power = read_u64(&format!("{}/power_now", bat_path))
        .or_else(|| {
            let current = read_u64(&format!("{}/current_now", bat_path))?;
            let voltage = read_u64(&format!("{}/voltage_now", bat_path))?;
            Some(current * voltage / 1_000_000)
        })?;

    let status = read_string(&format!("{}/status", bat_path))
        .unwrap_or_else(|| "Unknown".to_string());

    let energy_remaining = match status.as_str() {
        "Charging" => {
            if energy > energy_full { 0 } else { energy_full - energy }
        }
        "Full" => 0,
        _ => energy,
    };

    let capacity = match read_u64(&format!("{}/capacity", bat_path)) {
        Some(c) => c as f64,
        None if energy_full > 0 => (energy as f64 / energy_full as f64) * 100.0,
        None => 0.0,
    };

    Some(BatterySample {
        power: power as f64,
        energy_remaining: energy_remaining as f64,
        status,
        capacity,
    })
}

fn compute_minutes_remaining(
    latest: &BatterySample,
    trend: &TrendData,
) -> f64 {
    if latest.energy_remaining <= 0.0 {
        return 0.0;
    }
    if latest.power <= 0.0 {
        return f64::INFINITY;
    }

    if trend.samples.len() < 5 {
        let raw_hours = latest.energy_remaining / latest.power;
        return raw_hours * 60.0;
    }

    let smooth_power = trend.smoothed_power().unwrap_or(latest.power);

    if smooth_power <= 0.0 {
        let raw_hours = latest.energy_remaining / latest.power;
        return raw_hours * 60.0;
    }

    let trend_minutes = (latest.energy_remaining / smooth_power) * 60.0;
    let raw_minutes = (latest.energy_remaining / latest.power) * 60.0;

    let slope = trend.power_trend_slope();
    let predicted_power = (smooth_power * (1.0 + slope * 15.0)).max(smooth_power * 0.5);
    let predicted_minutes = (latest.energy_remaining / predicted_power) * 60.0;

    0.70 * trend_minutes + 0.15 * raw_minutes + 0.15 * predicted_minutes
}

fn power_status(status: &str) -> &str {
    match status {
        "Charging" => "Charging",
        "Discharging" => "Draining",
        "Full" => "Full",
        _ => "Unknown",
    }
}

fn format_time(total_minutes: f64, status: &str, fmt: &str, capacity: f64) -> String {
    let pstatus = power_status(status);

    if total_minutes == f64::INFINITY {
        return fmt.replace("%m", "inf")
            .replace("%h", "inf")
            .replace("%d", "inf")
            .replace("%D", "inf")
            .replace("%H", "00")
            .replace("%M", "00")
            .replace("%S", "00")
            .replace("%s", "0")
            .replace("%p", pstatus)
            .replace("%c", &format!("{:.0}", capacity))
            .replace("%C", &format!("{:03}", capacity as u64))
            .replace("%%", "%");
    }

    let total_min = total_minutes.round() as u64;
    let days = total_min / 1440;
    let hours = (total_min % 1440) / 60;
    let mins = total_min % 60;
    let total_hours_f = (total_min as f64) / 60.0;
    let total_days_f = (total_min as f64) / 1440.0;

    let mut out = String::with_capacity(fmt.len());
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('m') => out.push_str(&total_min.to_string()),
            Some('h') => out.push_str(&format!("{:.1}", total_hours_f)),
            Some('d') => out.push_str(&format!("{:02}", days)),
            Some('D') => out.push_str(&format!("{:.1}", total_days_f)),
            Some('H') => out.push_str(&format!("{:02}", hours)),
            Some('M') => out.push_str(&format!("{:02}", mins)),
            Some('S') => out.push_str("00"),
            Some('s') => out.push('0'),
            Some('p') => out.push_str(pstatus),
            Some('c') => out.push_str(&format!("{:.0}", capacity)),
            Some('C') => out.push_str(&format!("{:03}", capacity as u64)),
            Some('%') => out.push('%'),
            Some(x) => { out.push('%'); out.push(x); }
            None => out.push('%'),
        }
    }
    out
}

fn run_daemon() -> Result<(), String> {
    fs::create_dir_all(RUNTIME_DIR)
        .map_err(|e| format!("could not create runtime directory {}: {}", RUNTIME_DIR, e))?;

    // Socket stealing: if socket exists, check if a live daemon is behind it
    if Path::new(SOCKET_PATH).exists() {
        match UnixStream::connect(SOCKET_PATH) {
            Ok(mut stream) => {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let _ = stream.write_all(b"%m");
                let _ = stream.shutdown(std::net::Shutdown::Write);
                let mut buf = [0u8; 16];
                match stream.read(&mut buf) {
                    Ok(n) if n > 0 => {
                        eprintln!("daemon already running on {}", SOCKET_PATH);
                        return Ok(());
                    }
                    _ => {
                        eprintln!("warning: stale socket detected, stealing {}", SOCKET_PATH);
                        fs::remove_file(SOCKET_PATH)
                            .map_err(|e| format!("could not remove stale socket: {}", e))?;
                    }
                }
            }
            Err(_) => {
                eprintln!("warning: stale socket detected, stealing {}", SOCKET_PATH);
                fs::remove_file(SOCKET_PATH)
                    .map_err(|e| format!("could not remove stale socket: {}", e))?;
            }
        }
    }

    // Signal handling for graceful shutdown
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();
    ctrlc::set_handler(move || {
        r.store(false, Ordering::SeqCst);
    }).map_err(|e| format!("could not set signal handler: {}", e))?;

    let trend = Arc::new(Mutex::new(TrendData::new()));

    let trend_clone = Arc::clone(&trend);
    thread::spawn(move || {
        let mut current_bat: Option<String> = None;
        loop {
            if current_bat.is_none() {
                current_bat = find_battery();
                if current_bat.is_none() {
                    thread::sleep(BATTERY_RESCAN_INTERVAL);
                    continue;
                }
            }

            if let Some(ref bat_path) = current_bat {
                match read_battery(bat_path) {
                    Some(sample) => {
                        let mut t = trend_clone.lock().unwrap();
                        t.push(sample);
                    }
                    None => {
                        current_bat = None;
                        thread::sleep(BATTERY_RESCAN_INTERVAL);
                        continue;
                    }
                }
            }

            thread::sleep(POLL_INTERVAL);
        }
    });

    let listener = UnixListener::bind(SOCKET_PATH)
        .map_err(|e| format!("bind: {}", e))?;

    fs::set_permissions(SOCKET_PATH, PermissionsExt::from_mode(0o666))
        .map_err(|e| format!("could not set socket permissions: {}", e))?;

    listener.set_nonblocking(true)
        .map_err(|e| format!("set nonblocking: {}", e))?;

    let trend_clone = Arc::clone(&trend);
    while running.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                if let Err(e) = stream.set_read_timeout(Some(Duration::from_secs(5))) {
                    eprintln!("warning: could not set read timeout: {}", e);
                }
                let t = trend_clone.clone();
                thread::spawn(move || handle_client(stream, &t));
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                if running.load(Ordering::SeqCst) {
                    eprintln!("accept error: {}", e);
                }
                break;
            }
        }
    }

    // Clean up the socket on graceful shutdown
    let _ = fs::remove_file(SOCKET_PATH);

    Ok(())
}

fn handle_client(mut stream: UnixStream, trend: &Arc<Mutex<TrendData>>) {
    let mut buf = vec![0u8; MAX_REQUEST_SIZE];
    let n = match stream.read(&mut buf) {
        Ok(0) => return,
        Ok(n) => n,
        Err(_) => return,
    };
    buf.truncate(n);

    let raw = std::str::from_utf8(&buf).unwrap_or("%m");
    let (fmt, include_charging) = if let Some(s) = raw.strip_prefix('+') {
        (s, true)
    } else {
        (raw, false)
    };
    let fmt = if fmt.is_empty() { "%m" } else { fmt };

    let (latest, trend_data) = {
        let t = trend.lock().unwrap();
        (t.latest().cloned(), TrendData { samples: t.samples.clone() })
    };

    let response = match latest {
        Some(ref sample) if sample.status == "Charging" && !include_charging => {
            format_time(0.0, &sample.status, fmt, sample.capacity)
        }
        Some(ref sample) if sample.status == "Full" && !include_charging => {
            format_time(0.0, &sample.status, fmt, sample.capacity)
        }
        Some(ref sample) => {
            let mins = compute_minutes_remaining(sample, &trend_data);
            format_time(mins, &sample.status, fmt, sample.capacity)
        }
        None => "no data".to_string(),
    };

    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

fn run_monitor(fmt: &str, include_charging: bool) {
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();
    ctrlc::set_handler(move || {
        r.store(false, Ordering::SeqCst);
    }).expect("could not set signal handler");

    while running.load(Ordering::SeqCst) {
        match query_socket(fmt, include_charging) {
            Ok(resp) => {
                print!("\r\x1b[K{}", resp);
                std::io::stdout().flush().ok();
            }
            Err(e) => {
                eprint!("\r\x1b[Kerror: {}", e);
                break;
            }
        }
        thread::sleep(Duration::from_secs(1));
    }
    println!();
}

fn query_socket(fmt: &str, include_charging: bool) -> Result<String, String> {
    let mut stream = UnixStream::connect(SOCKET_PATH)
        .map_err(|e| format!("connect: {} (is the daemon running?)", e))?;
    let payload = if include_charging {
        format!("+{}", fmt)
    } else {
        fmt.to_string()
    };
    stream.write_all(payload.as_bytes())
        .map_err(|e| format!("write: {}", e))?;
    stream.shutdown(std::net::Shutdown::Write)
        .map_err(|e| format!("shutdown: {}", e))?;

    let mut response = String::new();
    let mut reader = BufReader::new(&mut stream);
    reader.read_to_string(&mut response)
        .map_err(|e| format!("read: {}", e))?;
    Ok(response.trim().to_string())
}
