//! Replay a trace recorded by prototype/testing/trace.py and compare the
//! CPU's registers with Unicorn's after every instruction.
//!
//!     lockstep <rom> <trace dir>

use std::collections::VecDeque;
use std::process::exit;
use std::sync::Arc;

use tg18::cpu::FLAG_T;
use tg18::machine::IRQ_VECTOR;
use tg18::snapshot::Snapshot;
use tg18::Machine;

const IRQ: u8 = 1;
const REG: u8 = 2;
const MEM: u8 = 3;
const MMIO: u8 = 4;

enum Event {
    Irq,
    Reg(usize, u32),
    Mem(u32, Vec<u8>),
}

fn inflate(path: &str) -> Vec<u8> {
    let data = std::fs::read(path).unwrap_or_else(|e| {
        eprintln!("{}: {}", path, e);
        exit(1)
    });
    miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&data, usize::MAX).unwrap_or_else(|e| {
        eprintln!("{}: {:?}", path, e);
        exit(1)
    })
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        eprintln!("usage: lockstep <rom> <trace dir>");
        exit(2);
    }
    let image = Arc::new(std::fs::read(&args[0]).expect("rom"));
    let dir = &args[1];
    let mut m = Machine::new(image.clone(), &image, 0.0);
    Snapshot::load(&format!("{}/start.t18s", dir))
        .and_then(|s| s.restore(&mut m))
        .unwrap_or_else(|e| {
            eprintln!("start.t18s: {}", e);
            exit(1)
        });

    let regs = inflate(&format!("{}/regs.bin", dir));
    let n = regs.len() / 144;
    let entry = |k: usize| -> [u64; 18] {
        let mut e = [0u64; 18];
        for (i, v) in e.iter_mut().enumerate() {
            let o = k * 144 + i * 8;
            *v = u64::from_le_bytes(regs[o..o + 8].try_into().unwrap());
        }
        e
    };

    // events, grouped by the instruction index they come before
    let ev = inflate(&format!("{}/events.bin", dir));
    let mut events: Vec<(usize, Event)> = Vec::new();
    let mut reads = VecDeque::new();
    let mut p = 0;
    let u32_at = |b: &[u8], o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    while p < ev.len() {
        let idx = u64::from_le_bytes(ev[p..p + 8].try_into().unwrap()) as usize;
        let kind = ev[p + 8];
        p += 9;
        match kind {
            IRQ => events.push((idx, Event::Irq)),
            REG => {
                events.push((idx, Event::Reg(ev[p] as usize, u32_at(&ev, p + 1))));
                p += 5;
            }
            MEM => {
                let addr = u32_at(&ev, p);
                let len = u32_at(&ev, p + 4) as usize;
                events.push((idx, Event::Mem(addr, ev[p + 8..p + 8 + len].to_vec())));
                p += 8 + len;
            }
            MMIO => {
                reads.push_back((u32_at(&ev, p), u32_at(&ev, p + 5)));
                p += 9;
            }
            _ => {
                eprintln!("bad event kind {} at {}", kind, p);
                exit(1);
            }
        }
    }
    println!("{} instructions, {} events, {} hardware reads", n, events.len(), reads.len());
    m.sys.replay_reads = Some(reads);

    let mut e = 0;
    let mut recent: VecDeque<(u32, u32)> = VecDeque::new();
    let mut steps = 0u64;
    let mut phantoms = 0u64;
    for k in 0..n {
        while e < events.len() && events[e].0 == k {
            match &events[e].1 {
                Event::Irq => m.cpu.enter_irq(IRQ_VECTOR),
                Event::Reg(r, v) => match *r {
                    15 => {
                        // Unicorn: bit 0 of a written PC selects Thumb
                        if v & 1 != 0 {
                            m.cpu.cpsr |= FLAG_T;
                        }
                        m.cpu.r[15] = v & !1;
                    }
                    16 => m.cpu.write_cpsr(*v),
                    r => m.cpu.r[r] = *v,
                },
                Event::Mem(a, d) => m.sys.poke(*a, d),
            }
            e += 1;
        }
        let want = entry(k);
        let mut got = [0u64; 17];
        for i in 0..16 {
            got[i] = m.cpu.r[i] as u64;
        }
        got[16] = m.cpu.cpsr as u64;
        let mut wanted = [0u64; 17];
        wanted.copy_from_slice(&want[..17]);
        wanted[15] &= !1;
        if got != wanted {
            println!("\nMISMATCH before instruction {} (after {} steps)", k, steps);
            let names = ["r0", "r1", "r2", "r3", "r4", "r5", "r6", "r7", "r8", "r9", "r10", "r11", "r12", "sp", "lr", "pc", "cpsr"];
            for i in 0..17 {
                let mark = if got[i] != wanted[i] { "  <--" } else { "" };
                println!("  {:>4}  rust {:08X}  unicorn {:08X}{}", names[i], got[i], wanted[i], mark);
            }
            println!("last instructions (pc, opcode), oldest first:");
            for (pc, op) in &recent {
                println!("  {:08X}  {:08X}", pc, op);
            }
            exit(1);
        }
        if let Some(err) = &m.sys.replay_error {
            println!("\nhardware read mismatch before instruction {}: {}", k, err);
            exit(1);
        }
        // Unicorn called the hook for an instruction that a hook then
        // replaced (the PC was moved before it ran): nothing to execute
        let replaced_later = events[e..].iter().take_while(|x| x.0 == k + 1).any(|x| matches!(x.1, Event::Reg(15, _)));
        if want[17] as u32 & !1 != want[15] as u32 & !1 || replaced_later {
            phantoms += 1;
            continue;
        }
        let pc = m.cpu.r[15];
        let op = if m.cpu.thumb() { m.sys.peek8(pc) as u32 | (m.sys.peek8(pc + 1) as u32) << 8 } else { m.sys.peek32(pc) };
        recent.push_back((pc, op));
        if recent.len() > 12 {
            recent.pop_front();
        }
        m.cpu.step(&mut m.sys);
        steps += 1;
        if let Some(f) = m.cpu.fault {
            println!("\nCPU fault at instruction {}: {:?}", k, f);
            exit(1);
        }
    }
    let left = m.sys.replay_reads.as_ref().map_or(0, |q| q.len());
    println!("OK: {} instructions identical ({} replaced by hooks), {} hardware reads left over", steps, phantoms, left);
}
