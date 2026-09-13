# batteryremainingd

A lightweight, zero-dependency daemon for accurate battery time remaining estimation on Linux.

## Features

- **Smart power prediction**: Weighted moving average with trend analysis prevents jittery estimates
- **Minimal overhead**: Single daemon process serving multiple clients via Unix socket
- **Flexible output**: Customizable format strings for status bars, terminals, and dashboards
- **Dual mode**: Query daemon on-demand or continuous monitoring with live updates
- **Zero dependencies**: Only `ctrlc` for signal handling; no bloat
- **Hardware compatible**: Works with both legacy (`charge_now`) and modern (`energy_now`) sysfs interfaces
- **Systemd integration**: Ready-to-use service file included

## Installation

### Arch Linux
```bash
git clone https://github.com/Enderman-brewer/batteryremainingd.git
cd batteryremainingd
makepkg -si
```

### Manual Build
```bash
cargo build --release
sudo install -Dm755 target/release/batteryremainingd /usr/bin/batteryremainingd
sudo install -Dm644 batteryremainingd.service /usr/lib/systemd/system/batteryremainingd.service
sudo systemctl daemon-reload
sudo systemctl enable --now batteryremainingd
```

## Usage

### Start the daemon
```bash
batteryremainingd --daemon
# or
systemctl start batteryremainingd
```

### Query current battery status
```bash
# Time remaining in minutes (default)
batteryremainingd -f '%m'
# Output: 187

# Time remaining as HH:MM
batteryremainingd -f '%H:%M'
# Output: 03:07

# Full status with capacity
batteryremainingd -f '[%p] %dd %Hh %Mm | %c%%'
# Output: [Draining] 00d 03h 07m | 72%

# When charging, show time-to-full
batteryremainingd -c -f 'Charging: %H:%M to full | %c%%'
```

### Continuous monitoring
```bash
# Monitor with default format
batteryremainingd -m
# Output: [STATUS] Draining | [TIME] 03:07

# Custom format while monitoring
batteryremainingd -m -f '[%p] Battery: %c%% | Remaining: %Hh %Mm'

# Show charging time
batteryremainingd -m -c
```

### Combined flags
Short flags can be combined:
```bash
batteryremainingd -mc     # Monitor with charging time included
batteryremainingd -cm -f '%H:%M'  # Monitor with custom format
```

## Format Specifiers

| Specifier | Output | Example |
|-----------|--------|---------|
| `%p` | Power status | `Charging`, `Draining`, `Full`, `Unknown` |
| `%m` | Total minutes | `187` |
| `%h` | Total hours (float) | `3.1` |
| `%d` | Days component (zero-padded) | `00` |
| `%D` | Total days (float) | `0.1` |
| `%H` | Hours component (zero-padded) | `03` |
| `%M` | Minutes component (zero-padded) | `07` |
| `%S` | Seconds component (always 00) | `00` |
| `%s` | Total seconds (always 0) | `0` |
| `%c` | Battery capacity (0-100) | `72` |
| `%C` | Battery capacity zero-padded (3 digits) | `072` |
| `%%` | Literal percent sign | `%` |

## Examples

### Status bar integration

**Polybar:**
```ini
[module/battery]
type = custom/script
exec = batteryremainingd -f '%c%% | %Hh %Mm'
interval = 5
```

**i3blocks:**
```
[battery]
command=batteryremainingd -f '%c%% (%Hh %Mm)'
interval=5
```

### Terminal prompt

Add to your shell config (`.bashrc`, `.zshrc`, etc.):
```bash
battery() {
  batteryremainingd -f "[%p] %c%% | %Hh %Mm remaining"
}
```

Then use `battery` in your prompt or call it directly.

### Dashboard script

```bash
#!/bin/bash
echo "=== System Status ==="
echo "Time: $(date '+%H:%M')"
echo "Battery: $(batteryremainingd -c -f '%p | %c%% (%Hh %Mm)')"
echo "Uptime: $(uptime -p)"
```

## How It Works

### Battery Reading
The daemon reads from Linux sysfs (`/sys/class/power_supply/`):
- **Energy/Charge**: Current and full capacity (joules or coulombs)
- **Power/Current & Voltage**: To compute current draw and normalize across battery types
- **Status**: Charging, discharging, or full

### Time Estimation
Rather than a naive division of energy by power, the daemon:

1. **Maintains a 60-sample rolling window** (60 seconds of history)
2. **Applies weighted averaging** to smooth power fluctuations—newer samples weighted higher
3. **Computes a trend slope** to detect if power draw is increasing or decreasing
4. **Blends three estimates**:
   - 70% from smoothed power trend
   - 15% from current instantaneous power
   - 15% from predicted power (accounting for trend direction)

This prevents the "flickering" effect where estimates jump around as usage patterns shift.

### Daemon Architecture
- **Single daemon process** reduces overhead—one `/sys` reader, one listening socket
- **Unix socket IPC** (`/run/batteryremainingd/batteryremainingd.sock`) for client communication
- **Non-blocking accept loop** with 100ms sleep prevents busy-waiting
- **Graceful shutdown** via SIGTERM/SIGINT, removes socket cleanly

## Architecture

```
Client 1 ─┐
Client 2 ─┼─> Unix Socket ─> Daemon (reads /sys, maintains trend data) ─> /sys/class/power_supply
Client 3 ─┘
```

## Configuration

### Systemd service

Edit `/usr/lib/systemd/system/batteryremainingd.service` to customize:
- **User**: By default runs as `batteryremaining_duser`; change if needed
- **RestartSec**: Restart delay (default 3s)
- **Type**: Set to `simple` for foreground daemon

After changes:
```bash
sudo systemctl daemon-reload
sudo systemctl restart batteryremainingd
```

## Troubleshooting

### Daemon won't start
```bash
systemctl status batteryremainingd
journalctl -u batteryremainingd -n 20
```

### "is the daemon running?" error
```bash
# Check if daemon is active
systemctl is-active batteryremainingd

# Start it if not running
systemctl start batteryremainingd

# Verify socket exists
ls -l /run/batteryremainingd/batteryremainingd.sock
```

### Time estimate seems wrong
- Daemon needs ~5 samples (5+ seconds) to build trend data; early estimates use raw power
- Highly variable power draw (lots of I/O, CPU bursts) makes prediction harder
- **Solution**: Monitor for 30+ seconds to let trend stabilize

### Battery not detected
Check `/sys/class/power_supply/`:
```bash
ls /sys/class/power_supply/
# Look for something like BAT0, BAT1, or similar
cat /sys/class/power_supply/BAT0/type
# Should output "Battery"
```

If no battery found, the daemon will keep rescanning every 10 seconds.

## Performance

- **Memory**: ~2MB resident (includes 60-sample history buffer)
- **CPU**: <0.1% idle (wakes every 100ms when no clients connected)
- **Latency**: <1ms query response time (O(1) computation)

## License

See LICENSE file in repository.

## Contributing

Issues and pull requests welcome. Focus areas:
- Additional format specifiers (e.g., status icons)
- Non-Linux port support (macOS, BSD)
- Additional sysfs interface compatibility

## See Also

- `acpi` - Simple ACPI battery info tool (no daemon)
- `upower` - Full-featured power management daemon (heavier)
- `tlp` - Power management for Linux laptops (full suite)
