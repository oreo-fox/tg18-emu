//! Settings, the ROM folder and per-ROM saves (same layout as the prototype):
//!
//!   settings.json            window settings (shared with the prototype)
//!   roms/                    the user's ROM dumps (default ROM folder)
//!   saves/<rom name>/game.flash (+ .json)   the toy's own flash save and clock
//!                            (<rom name>: the version's usual file name, see
//!                            identity(), so renamed dumps keep their saves)
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
    /// A ROM folder the user chose; None = the roms folder next to the program.
    pub rom_dir: Option<String>,
    pub last_rom: Option<String>,
    pub volume: u32,
    pub muted: bool,
    pub scale: u32,
    pub autosave_minutes: u32,
    pub never_sleep: bool,
    pub pause_time_when_closed: bool,
    /// Window colour: "pink", "blue", "green", "yellow" or "lilac".
    pub theme: String,
    /// Desktop mode: the toy sits on the desktop instead of in a window.
    pub desk_mode: bool,
    /// Where the desktop toy was left (screen pixels), None = not placed yet.
    pub desk_x: Option<i32>,
    pub desk_y: Option<i32>,
    /// Screen size of the desktop toy (2x-4x).
    pub desk_scale: u32,
    pub desk_on_top: bool,
    /// Keep the toy awake in desktop mode too. Off: it sleeps like the real
    /// toy (screen off, almost no CPU) and a button wakes it.
    pub desk_never_sleep: bool,
    /// Letters A, B, C on the buttons (off: plain buttons).
    pub button_labels: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            rom_dir: None,
            last_rom: None,
            volume: 20,
            muted: false,
            scale: 3,
            autosave_minutes: 1,
            never_sleep: false,
            pause_time_when_closed: false,
            theme: "pink".to_string(),
            desk_mode: false,
            desk_x: None,
            desk_y: None,
            desk_scale: 2,
            desk_on_top: false,
            desk_never_sleep: false,
            button_labels: true,
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

    /// The folder ROMs are listed from.
    pub fn rom_folder(&self) -> PathBuf {
        self.rom_dir.as_ref().map(PathBuf::from).unwrap_or_else(roms_dir)
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

/// The default ROM folder, next to the program (empty in a release: the
/// user puts their own dumps there).
pub fn roms_dir() -> PathBuf {
    app_dir().join("roms")
}

/// True if both paths name the same folder.
pub fn same_dir(a: &Path, b: &Path) -> bool {
    a == b
        || match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
            (Ok(x), Ok(y)) => x == y,
            _ => false,
        }
}

pub fn is_rom(path: &Path) -> bool {
    use std::io::Read;
    let ok_size = std::fs::metadata(path).map(|m| m.len() as usize == tg18::FLASH_SIZE).unwrap_or(false);
    let mut head = [0u8; 4];
    ok_size
        && std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut head)).is_ok()
        && &head == b"SPII"
}

/// What a dump is: (display name, save folder name). Known versions are
/// recognised by their program code, whatever the file is called; others
/// go by the file name.
pub fn identity(filename: &str, image: &[u8]) -> (String, String) {
    identity_of_hash(filename, tg18::code_hash(image))
}

fn identity_of_hash(filename: &str, hash: u64) -> (String, String) {
    match tg18::roms::identify_hash(hash) {
        Some(k) => (k.display_name(), k.stem()),
        None => {
            let stem = Path::new(filename).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            (display_name(filename), stem)
        }
    }
}

/// Code fingerprints of the files looked at, by path, with the file's size
/// and time: the folder is rescanned every few seconds, the files are only
/// read again when they change.
type HashCache = std::collections::HashMap<PathBuf, (u64, std::time::SystemTime, u64)>;
static HASHES: std::sync::Mutex<Option<HashCache>> = std::sync::Mutex::new(None);

/// identity() of a dump on disk.
pub fn file_identity(path: &Path) -> Option<(String, String)> {
    use std::io::Read;
    let meta = std::fs::metadata(path).ok()?;
    let (len, time) = (meta.len(), meta.modified().ok()?);
    let mut cache = HASHES.lock().unwrap_or_else(|e| e.into_inner());
    let cache = cache.get_or_insert_with(Default::default);
    let hash = match cache.get(path) {
        Some(&(l, t, h)) if l == len && t == time => h,
        _ => {
            let mut code = vec![0u8; tg18::CODE_CHECK_LEN];
            std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut code)).ok()?;
            let h = tg18::code_hash(&code);
            cache.insert(path.to_path_buf(), (len, time, h));
            h
        }
    };
    let filename = path.file_name()?.to_string_lossy().to_string();
    Some(identity_of_hash(&filename, hash))
}

/// (file name, display name) of every tg18 image in folder, sorted by name.
pub fn list_roms(folder: &Path) -> Vec<(String, String)> {
    let mut roms: Vec<(String, String)> = std::fs::read_dir(folder)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().map_or(false, |x| x.eq_ignore_ascii_case("bin")) && is_rom(p))
                .filter_map(|p| Some((p.file_name()?.to_string_lossy().to_string(), file_identity(&p)?.0)))
                .collect()
        })
        .unwrap_or_default();
    // two copies of the same version: tell them apart by file name
    let names: Vec<String> = roms.iter().map(|r| r.1.clone()).collect();
    for r in roms.iter_mut() {
        if names.iter().filter(|n| **n == r.1).count() > 1 {
            r.1 = format!("{} \u{2013} {}", r.1, r.0);
        }
    }
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
    /// The saves of a ROM; `name` is its save folder name (see identity()).
    pub fn new(name: &str) -> SaveStore {
        let dir = saves_dir().join(name);
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
