//! Command-line runs, like the prototype's tg18emu.py:
//!
//!     tg18cli <flash.bin> [--seconds N | --insns N] [--press T:KEY ...]
//!         [--date "YYYY-MM-DD HH:MM"] [--save FILE] [--snapshot-in FILE]
//!         [--snapshot-out FILE] [--turbo N] [--never-sleep] [--no-idle-skip]
//!         [--screenshot PNG] [--wav FILE] [--trace-mmio] [--quiet]
//!         [--dump ADDR:LEN ...]   (print memory at the end, hex address)

use std::process::exit;
use std::sync::Arc;
use std::time::Instant;

use tg18::snapshot::Snapshot;
use tg18::{save, Key, Machine, Outcome, CPU_HZ};

fn usage() -> ! {
    eprintln!("{}", include_str!("tg18cli.rs").lines().take(7).map(|l| l.trim_start_matches("//!")).collect::<Vec<_>>().join("\n"));
    exit(2)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut image_path = None;
    let mut insns: u64 = 50_000_000;
    let mut presses: Vec<(f64, Key)> = Vec::new();
    let mut date = None;
    let mut save_path: Option<String> = None;
    let mut snap_in: Option<String> = None;
    let mut snap_out: Option<String> = None;
    let mut turbo = 1.0;
    let mut never_sleep = false;
    let mut idle_skip = true;
    let mut screenshot = "screen.png".to_string();
    let mut wav: Option<String> = None;
    let mut trace = false;
    let mut quiet = false;
    let mut dumps: Vec<(u32, u32)> = Vec::new();
    let mut i = 0;
    let next = |i: &mut usize| -> String {
        *i += 1;
        args.get(*i).cloned().unwrap_or_else(|| usage())
    };
    while i < args.len() {
        match args[i].as_str() {
            "--seconds" => insns = (next(&mut i).parse::<f64>().unwrap_or_else(|_| usage()) * CPU_HZ) as u64,
            "--insns" => insns = next(&mut i).parse().unwrap_or_else(|_| usage()),
            "--press" => {
                let spec = next(&mut i);
                let (t, keys) = spec.split_once(':').unwrap_or_else(|| usage());
                let t: f64 = t.parse().unwrap_or_else(|_| usage());
                for k in keys.split('+') {
                    presses.push((t, Key::parse(k).unwrap_or_else(|| usage())));
                }
            }
            "--date" => date = Some(next(&mut i)),
            "--save" => save_path = Some(next(&mut i)),
            "--snapshot-in" => snap_in = Some(next(&mut i)),
            "--snapshot-out" => snap_out = Some(next(&mut i)),
            "--turbo" => turbo = next(&mut i).parse().unwrap_or_else(|_| usage()),
            "--never-sleep" => never_sleep = true,
            "--no-idle-skip" => idle_skip = false,
            "--screenshot" => screenshot = next(&mut i),
            "--wav" => wav = Some(next(&mut i)),
            "--trace-mmio" => trace = true,
            "--quiet" => quiet = true,
            "--dump" => {
                let spec = next(&mut i);
                let (a, n) = spec.split_once(':').unwrap_or_else(|| usage());
                let a = u32::from_str_radix(a.trim_start_matches("0x"), 16).unwrap_or_else(|_| usage());
                dumps.push((a, n.parse().unwrap_or_else(|_| usage())));
            }
            "-h" | "--help" => usage(),
            a if !a.starts_with("--") && image_path.is_none() => image_path = Some(a.to_string()),
            _ => usage(),
        }
        i += 1;
    }
    let image_path = image_path.unwrap_or_else(|| usage());
    let image = std::fs::read(&image_path).unwrap_or_else(|e| {
        eprintln!("{}: {}", image_path, e);
        exit(1)
    });
    if !tg18::is_image(&image) {
        eprintln!("not a tg18 SPI image (missing SPII header)");
        exit(1);
    }
    let rtc = match &date {
        Some(d) => parse_date(d).unwrap_or_else(|| usage()),
        None => tg18::rtc_seconds_now(),
    };
    let image = Arc::new(image);
    let saved = match &save_path {
        Some(p) => save::load_flash(p, &image).unwrap_or_else(|e| {
            eprintln!("save {}: {}", p, e);
            exit(1)
        }),
        None => None,
    };
    let mut m = Machine::new(image.clone(), saved.as_deref().unwrap_or(&image), rtc);
    m.print_log = !quiet;
    m.sys.trace_mmio = trace;
    if saved.is_some() {
        let p = save_path.as_ref().unwrap();
        let resumed = date.is_none()
            && match save::read_meta(p) {
                Some(meta) => {
                    save::restore_rtc(&mut m, &meta, true);
                    true
                }
                None => false,
            };
        println!("booting from save {}{}", p, if resumed { " (clock resumed)" } else { "" });
    }
    if let Some(p) = &snap_in {
        let snap = Snapshot::load(p).unwrap_or_else(|e| {
            eprintln!("snapshot {}: {}", p, e);
            exit(1)
        });
        if let Err(e) = snap.restore(&mut m) {
            eprintln!("snapshot {}: {}", p, e);
            exit(1);
        }
        println!("resumed snapshot {} at {:.3} s", p, m.now());
    }
    m.sys.turbo = turbo;
    m.idle_skip = idle_skip;
    m.never_sleep = never_sleep;
    let t0 = m.now();
    for (t, k) in presses {
        m.press(t0 + t, k);
    }
    let end = m.executed + insns;
    let wall = Instant::now();
    let outcome = m.run_with_sleep(end);
    let secs = wall.elapsed().as_secs_f64();
    let emulated = (m.executed as f64 / CPU_HZ) - t0;
    println!(
        "\n=== result: {:?} after {:.3} s emulated in {:.2} s real ({:.1}x), pc={:08X} ===",
        outcome,
        emulated,
        secs,
        emulated / secs.max(1e-9),
        m.cpu.pc()
    );
    println!(
        "idle skipped {:.3} s, IRQs {}, printf lines {}, flash writes {}, LCD frames {}",
        m.idle_skipped as f64 / CPU_HZ,
        m.irqs,
        m.log.len(),
        m.sys.flash.writes,
        m.sys.lcd.frames
    );
    println!("idle probes [tried, no repeat, memory changed, hardware written, skipped]: {:?}", m.probe_stats);
    if outcome == Outcome::Crash {
        println!("fault: {:?}", m.fault());
    }
    let tod = m.sys.rtc_irq.tod(m.sys.game_time);
    println!("game time {:.0} s, time of day {:02}:{:02}", m.sys.game_time, tod / 3600, tod / 60 % 60);
    for (a, n) in dumps {
        let bytes: Vec<String> = (0..n).map(|k| format!("{:02X}", m.sys.peek8(a + k))).collect();
        println!("{:08X}: {}", a, bytes.join(" "));
    }
    tg18::png::write(&screenshot, 128, 128, &m.lcd_rgb(), 3).expect("screenshot");
    println!("final screen saved to {}", screenshot);
    if let Some(p) = &save_path {
        save::write_machine(&m, p, false, false).expect("save");
        println!("flash and clock saved to {}", p);
    }
    if let Some(p) = &snap_out {
        Snapshot::capture(&m).save(p).expect("snapshot");
        println!("snapshot written to {}", p);
    }
    if let Some(p) = &wav {
        let ev = &m.sys.sound.events;
        tg18::sound::write_wav(p, ev, t0, m.now(), 44100).expect("wav");
        println!("{} buzzer changes, sound written to {}", ev.len(), p);
    }
}

/// "YYYY-MM-DD HH:MM" -> RTC seconds.
fn parse_date(s: &str) -> Option<f64> {
    let (d, t) = s.trim().split_once(' ')?;
    let mut d = d.split('-').map(|x| x.parse::<i64>());
    let (y, mo, da) = (d.next()?.ok()?, d.next()?.ok()?, d.next()?.ok()?);
    let (h, mi) = t.split_once(':')?;
    Some(tg18::rtc_seconds(y, mo as u32, da as u32, h.parse().ok()?, mi.parse().ok()?, 0))
}
