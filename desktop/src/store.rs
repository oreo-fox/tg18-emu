//! Settings, the ROM folder and per-ROM saves (same layout as the prototype):
//!
//!   settings.json            window settings (shared with the prototype)
//!   saves/<rom name>/game.flash (+ .json)   the toy's own flash save and clock
//!   saves/<rom name>/autosave.t18s          snapshot, every few minutes
//!   saves/<rom name>/slot1-3.t18s           manual save slots
//!
//! game.flash works in both versions; snapshots are per version (the
//! prototype's are .snap files).

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use serde::{Deserialize, Serialize};

pub const SLOTS: usize = 3;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Settings {
    pub rom_dir: Option<String>,
    pub last_rom: Option<String>,
    pub volume: u32,
    pub muted: bool,
    pub scale: u32,
    pub autosave_minutes: u32,
    pub never_sleep: bool,
    pub pause_time_when_closed: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            rom_dir: None,
            last_rom: None,
            volume: 60,
            muted: false,
            scale: 4,
            autosave_minutes: 1,
            never_sleep: true,
            pause_time_when_closed: false,
        }
    }
}

/// Where settings and saves live: the project folder when the program runs
/// from its build folder (target/release), else next to the program.
/// TG18_HOME overrides it (for tests).
pub fn app_dir() -> PathBuf {
    if let Ok(h) = std::env::var("TG18_HOME") {
        return PathBuf::from(h);
    }
    let exe = std::env::current_exe().unwrap_or_default();
    let dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    for up in dir.ancestors().take(4) {
        if up.join("Cargo.toml").exists() && up.join("core").exists() {
            return up.to_path_buf();
        }
    }
    dir
}

impl Settings {
    pub fn path() -> PathBuf {
        app_dir().join("settings.json")
    }

    pub fn load() -> Settings {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn store(&self) {
        let p = Self::path();
        let tmp = p.with_extension("json.tmp");
        if std::fs::write(&tmp, serde_json::to_string_pretty(self).unwrap()).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
    }
}

pub fn saves_dir() -> PathBuf {
    app_dir().join("saves")
}

pub fn is_rom(path: &Path) -> bool {
    use std::io::Read;
    let ok_size = std::fs::metadata(path).map(|m| m.len() as usize == tg18::FLASH_SIZE).unwrap_or(false);
    let mut head = [0u8; 4];
    ok_size
        && std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut head)).is_ok()
        && &head == b"SPII"
}

/// (file name, display name) of every tg18 image in folder, sorted by name.
pub fn list_roms(folder: Option<&str>) -> Vec<(String, String)> {
    let folder = match folder {
        Some(f) => f,
        None => return Vec::new(),
    };
    let mut roms: Vec<(String, String)> = std::fs::read_dir(folder)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().map_or(false, |x| x.eq_ignore_ascii_case("bin")) && is_rom(p))
                .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
                .map(|n| {
                    let d = display_name(&n);
                    (n, d)
                })
                .collect()
        })
        .unwrap_or_default();
    roms.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
    roms
}

/// 'fw_tg18_en_v063_2020-02-13_wondergarden.bin' -> 'Wonder Garden (EN v063)'.
pub fn display_name(filename: &str) -> String {
    let stem = filename.strip_suffix(".bin").unwrap_or(filename);
    let parts: Vec<&str> = stem.splitn(6, '_').collect();
    if parts.len() == 6 && parts[0] == "fw" && parts[1] == "tg18" && parts[3].starts_with('v') {
        let name = parts[5];
        let nice = if name.eq_ignore_ascii_case("wondergarden") {
            "Wonder Garden".to_string()
        } else {
            name.split('_')
                .map(|w| {
                    let mut c = w.chars();
                    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
                })
                .collect::<Vec<_>>()
                .join(" ")
        };
        return format!("{} ({} v{})", nice, parts[2].to_uppercase(), &parts[3][1..]);
    }
    stem.to_string()
}

pub enum Newest {
    Flash(Option<tg18::save::SaveMeta>),
    Snapshot,
    Nothing,
}

/// The save files of one ROM.
pub struct SaveStore {
    pub dir: PathBuf,
    pub flash: String,
    pub autosave: String,
}

fn mtime(p: &str) -> Option<f64> {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
}

impl SaveStore {
    pub fn new(rom_path: &Path) -> SaveStore {
        let stem = rom_path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let dir = saves_dir().join(stem);
        let _ = std::fs::create_dir_all(&dir);
        let s = |n: &str| dir.join(n).to_string_lossy().to_string();
        SaveStore { flash: s("game.flash"), autosave: s("autosave.t18s"), dir }
    }

    pub fn slot(&self, n: usize) -> String {
        self.dir.join(format!("slot{}.t18s", n)).to_string_lossy().to_string()
    }

    pub fn slot_time(&self, n: usize) -> Option<f64> {
        mtime(&self.slot(n))
    }

    /// Adopt a save made before per-ROM folders (saves/*.flash) if it fits.
    pub fn import_legacy(&self, image: &[u8]) -> Option<String> {
        if Path::new(&self.flash).exists() || Path::new(&self.autosave).exists() {
            return None;
        }
        let mut found: Vec<PathBuf> = std::fs::read_dir(saves_dir())
            .ok()?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().map_or(false, |x| x == "flash"))
            .collect();
        found.sort();
        for p in found {
            let ps = p.to_string_lossy().to_string();
            if matches!(tg18::save::load_flash(&ps, image), Ok(Some(_))) {
                std::fs::copy(&p, &self.flash).ok()?;
                let _ = std::fs::copy(format!("{}.json", ps), format!("{}.json", self.flash));
                return Some(p.file_name()?.to_string_lossy().to_string());
            }
        }
        None
    }

    /// Which state to resume. A clean flash save (written after the device
    /// went to sleep on close) and the autosave compete by age; a flash save
    /// written while awake is only used if there's nothing else.
    pub fn newest(&self) -> Newest {
        let meta = tg18::save::read_meta(&self.flash);
        let snap = mtime(&self.autosave);
        let clean = meta.as_ref().map_or(false, |m| m.clean);
        if clean && snap.map_or(true, |s| meta.as_ref().unwrap().saved_at >= s) {
            return Newest::Flash(meta);
        }
        if snap.is_some() {
            return Newest::Snapshot;
        }
        if meta.is_some() || Path::new(&self.flash).exists() {
            return Newest::Flash(meta);
        }
        Newest::Nothing
    }
}

/// Writes saves on a background thread, one at a time, in order.
pub struct Writer {
    jobs: mpsc::Sender<Box<dyn FnOnce() -> Result<(), String> + Send>>,
    errors: mpsc::Receiver<String>,
}

impl Writer {
    pub fn new() -> Writer {
        let (tx, rx) = mpsc::channel::<Box<dyn FnOnce() -> Result<(), String> + Send>>();
        let (etx, erx) = mpsc::channel();
        std::thread::spawn(move || {
            for job in rx {
                if let Err(e) = job() {
                    let _ = etx.send(e);
                }
            }
        });
        Writer { jobs: tx, errors: erx }
    }

    pub fn submit<F: FnOnce() -> Result<(), String> + Send + 'static>(&self, f: F) {
        let _ = self.jobs.send(Box::new(f));
    }

    /// Block until everything submitted so far is written.
    pub fn wait(&self) {
        let (tx, rx) = mpsc::channel();
        self.submit(move || {
            let _ = tx.send(());
            Ok(())
        });
        let _ = rx.recv();
    }

    pub fn errors(&self) -> Vec<String> {
        self.errors.try_iter().collect()
    }
}
