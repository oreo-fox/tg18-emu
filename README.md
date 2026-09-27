# tg18-emu

Experimental emulator for Tamagotchi Meets / Tamagotchi On ("tg18") firmware,
built as a feasibility study. No firmware is included; you need your own
8 MiB SPI flash dump.

Status: all nine known dumps boot to the clock-setting screen. The Wonder
Garden (EN v063) image has been played through setup (clock, birthday, name)
to a hatched baby, feeding and the menus, with saves and deep sleep/wake working.

## Legal

This project contains no Bandai firmware, graphics or other copyrighted
material, only independently written emulator code and hardware notes. Do not
open issues or pull requests containing firmware dumps, save files, snapshots
or screenshots. Tamagotchi is a trademark of Bandai; this project is not
affiliated with or endorsed by Bandai.

The emulator code is released under the MIT License (see `LICENSE`).

## Usage

    pip install -r requirements.txt
    python tg18emu.py <flash.bin> --insns 800000000 --frames frames --press 6.0:A --press 7.2:B

* `--seconds N`    emulated seconds to run (or `--insns N`)
* `--frames DIR`   save every distinct LCD frame as PNG (3x scale)
* `--press T:KEY`  press A, B or C at T seconds into this run; `A+C` presses both
* `--date "YYYY-MM-DD HH:MM"`  RTC start time (default: now, or the saved clock)
* `--save FILE`    persistent flash (the game's save data) plus `FILE.json` for
  the clock; boots from it if it exists and writes it back at the end. The clock
  keeps running while the emulator is closed, like a real device.
* `--snapshot-out FILE` / `--snapshot-in FILE`  freeze and resume the whole
  machine mid-game. Snapshots are Python pickles: only load ones you made.
* `--turbo N`      run the device clock N times faster (fast-forward game time)
* `--trace-mmio`   print the first access to every hardware register

Deep sleep is emulated: after about a minute without input the firmware arms
the RTC alarm and cuts power. The emulator then skips straight to the alarm
(or the next scripted button press) and cold-boots the CPU, keeping flash, the
RTC and its battery-backed registers, like the real chip. Sleep costs no
emulation time.

* `--no-idle-skip` always execute idle loops (for comparing behaviour)

Speed: idle loops are detected by running one more pass of the current loop;
if registers, internal RAM and hardware writes are unchanged, emulated time
jumps to the next interrupt or button event. Typical screens then run faster
than real time (about 1.5x on the baby screen); CPU-heavy stretches run at
about 0.4x, limited by Unicorn's instruction-count slicing in Python. Deep
sleep costs nothing. The emulator never writes to the dump file. Saves and
snapshots refuse to load with a different ROM version.

## Controls learned so far (EN Wonder Garden)

* Clock / birthday setup: A changes the blinking field, B confirms it.
* Name entry: A/C move the cursor right/left, B edits a slot. In the picker,
  A cycles pages (letters, digits, symbols), B picks. Moving past the last slot
  opens CONFIRM? YES/NO.
* Main screen: A opens the 10-icon menu and moves between icons (it remembers
  the last one), B selects, C goes back. Icons: Profile Setting, Meal&Snack,
  Bathroom, Connection, (house), Explore My Town, Travel to Other Town,
  Item Box, Notebook, First Aid Kit.
* Feeding: Meal&Snack > Fridge > food > B. Status: Profile Setting > Profile,
  A flips pages.

## Hardware notes (GeneralPlus GPBT03-family SoC, ARM926-class core)

| Address | Block | Notes |
|---|---|---|
| 0x20000000 | SPI flash, memory-mapped | image runs in place; reset at 0x20000048 |
| 0xF8000000 | internal SRAM | .data/.bss/stacks, flash driver, ISRs |
| 0xC0000000 | GPIO | port n at +n*0x20; buttons A/B/C = port 1 bits 5/6/7, active high |
| 0xC0020000 | 6 timers | +0 ctrl (b15 pending W1C, b14 IRQ en, b13 run, clock sysclk/2 or sysclk/256 with b0), +8 reload, +0x10 count; IRQ 8 |
| 0xC0090000 | RTC register bus | +4 addr, +8 wdata, +0xC cmd (1 wr/2 rd), +0x10 ready, +0x14 rdata |
| 0xC00C0000 | ADC | +4 ctrl (b14 start, b15 done), +8 result; battery check; IRQ 29 |
| 0xC0150000 | SPI flash controller | manual mode: +0 b6 CS, +8 TX, +0xC RX, +4 b3 busy |
| 0xC0040000 | RTC interrupts | +0x54 status (W1C), +0x58 enable; bit 1 = game-time tick (1 Hz assumed); IRQ 1 |
| 0xD0000000 | clock/system | +0x3C clock source status, +0x78 bit 0 = power (cleared for deep sleep) |
| 0xD0100000 | interrupt controller | +0x28 pending IRQ number, +0x30 mask, +0x38 b0 disable |
| 0xD0500000 | LCD interface | 128x128 RGB565; +0x140 ctrl (0x81 cmd, 0xA1 data, 0xD1 DMA from +0x33C), +0x18C status (0x2000 vsync, 0x40 DMA done); IRQ 27 |

RTC: registers 0x30-0x35 are a 48-bit 32768 Hz counter (0 = 2007-12-31),
0x10-0x15 stage a new value, 0x20-0x25 are the wake alarm, 0x40/0x50 arm it,
and 0x80-0xF7 are battery-backed memory (validity marker "XYZ[", sleep state).
Sound is a square-wave buzzer on timer 4, driven by a note sequencer at 125 Hz.
The firmware's printf is compiled out; the emulator hooks it to show the log.
