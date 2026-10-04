//! The known firmware versions, recognised by the fingerprint of their
//! program code (`code_hash`), so a dump is named and finds its saves
//! whatever its file is called. The dumps contain no readable name or
//! version. Only fingerprints are listed here, no firmware.

use crate::code_hash;

pub struct KnownRom {
    /// `code_hash` of the dump.
    pub hash: u64,
    /// "en" or "jp".
    pub region: &'static str,
    pub version: u32,
    /// Build date as in the dumps' usual file names.
    pub date: &'static str,
    /// Short name as in the usual file names.
    pub slug: &'static str,
    pub name: &'static str,
}

pub const KNOWN: [KnownRom; 9] = [
    KnownRom { hash: 0xD780_5452_8833_EB90, region: "en", version: 57, date: "2019-05-13", slug: "fairy", name: "Fairy" },
    KnownRom { hash: 0x8313_24F3_B2FA_6768, region: "en", version: 58, date: "2019-05-13", slug: "magic", name: "Magic" },
    KnownRom { hash: 0xA38D_22B6_76C5_3108, region: "en", version: 63, date: "2020-02-13", slug: "wondergarden", name: "Wonder Garden" },
    KnownRom { hash: 0x7056_A91C_C05E_227F, region: "jp", version: 30, date: "2018-10-19", slug: "fairy", name: "Fairy" },
    KnownRom { hash: 0x4089_9249_C2E4_6D53, region: "jp", version: 31, date: "2018-10-19", slug: "magic", name: "Magic" },
    KnownRom { hash: 0xEA18_7BF0_CBFA_04F3, region: "jp", version: 47, date: "2019-05-28", slug: "fantasy", name: "Fantasy" },
    KnownRom { hash: 0xA6FF_6D03_28E7_0278, region: "jp", version: 55, date: "2019-03-10", slug: "pastel", name: "Pastel" },
    KnownRom { hash: 0x3A68_A53E_2DB1_64B2, region: "jp", version: 56, date: "2019-04-24", slug: "sanrio", name: "Sanrio" },
    KnownRom { hash: 0x28AD_63C3_4FA9_E369, region: "jp", version: 62, date: "2019-09-12", slug: "sweets", name: "Sweets" },
];

impl KnownRom {
    /// "Fairy (EN v057)"
    pub fn display_name(&self) -> String {
        format!("{} ({} v{:03})", self.name, self.region.to_uppercase(), self.version)
    }

    /// The dump's usual file name without ".bin", also used as the name of
    /// its save folder: "fw_tg18_en_v057_2019-05-13_fairy".
    pub fn stem(&self) -> String {
        format!("fw_tg18_{}_v{:03}_{}_{}", self.region, self.version, self.date, self.slug)
    }
}

/// Which known version a dump is, from its program code.
pub fn identify(image: &[u8]) -> Option<&'static KnownRom> {
    identify_hash(code_hash(image))
}

pub fn identify_hash(hash: u64) -> Option<&'static KnownRom> {
    KNOWN.iter().find(|k| k.hash == hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_fingerprints() {
        for (i, k) in KNOWN.iter().enumerate() {
            assert!(KNOWN[i + 1..].iter().all(|o| o.hash != k.hash && o.stem() != k.stem()));
        }
        let wg = identify_hash(0xA38D_22B6_76C5_3108).unwrap();
        assert_eq!(wg.display_name(), "Wonder Garden (EN v063)");
        assert_eq!(wg.stem(), "fw_tg18_en_v063_2020-02-13_wondergarden");
        assert!(identify(&[0u8; 16]).is_none());
    }
}
