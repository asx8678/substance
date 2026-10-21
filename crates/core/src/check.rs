//! Check identifiers, the enums that describe a check, and their spellings.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Every check in the catalog, in ID order.
///
/// The discriminants are positional, so `CheckId::B01 as usize == 0` and
/// `id as usize` is the registry index.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema, TS,
)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[repr(u8)]
pub enum CheckId {
    /// The `B01` check.
    B01,
    /// The `B02` check.
    B02,
    /// The `B03` check.
    B03,
    /// The `B04` check.
    B04,
    /// The `B05` check.
    B05,
    /// The `B06` check.
    B06,
    /// The `B07` check.
    B07,
    /// The `B08` check.
    B08,
    /// The `B09` check.
    B09,
    /// The `B10` check.
    B10,
    /// The `B11` check.
    B11,
    /// The `B12` check.
    B12,
    /// The `B13` check.
    B13,
    /// The `B14` check.
    B14,
    /// The `B15` check.
    B15,
    /// The `B16` check.
    B16,
    /// The `B17` check.
    B17,
    /// The `B18` check.
    B18,
    /// The `B19` check.
    B19,
    /// The `B20` check.
    B20,
    /// The `B21` check.
    B21,
    /// The `B22` check.
    B22,
    /// The `B23` check.
    B23,
    /// The `B24` check.
    B24,
    /// The `B25` check.
    B25,
    /// The `B26` check.
    B26,
    /// The `B27` check.
    B27,
    /// The `B28` check.
    B28,
    /// The `B29` check.
    B29,
    /// The `B30` check.
    B30,
    /// The `B31` check.
    B31,
    /// The `B32` check.
    B32,
    /// The `B33` check.
    B33,
    /// The `I01` check.
    I01,
    /// The `I02` check.
    I02,
    /// The `I03` check.
    I03,
    /// The `I04` check.
    I04,
    /// The `D01` check.
    D01,
    /// The `D02` check.
    D02,
    /// The `D03` check.
    D03,
    /// The `D04` check.
    D04,
    /// The `O01` check.
    O01,
    /// The `O02` check.
    O02,
    /// The `O03` check.
    O03,
    /// The `O04` check.
    O04,
    /// The `O05` check.
    O05,
    /// The `O06` check.
    O06,
    /// The `O07` check.
    O07,
    /// The `O08` check.
    O08,
    /// The `O09` check.
    O09,
    /// The `O10` check.
    O10,
    /// The `O11` check.
    O11,
    /// The `L01` check.
    L01,
    /// The `L02` check.
    L02,
    /// The `L03` check.
    L03,
    /// The `L04` check.
    L04,
    /// The `L05` check.
    L05,
    /// The `S01` check.
    S01,
    /// The `S02` check.
    S02,
    /// The `S03` check.
    S03,
    /// The `S04` check.
    S04,
    /// The `S05` check.
    S05,
    /// The `S06` check.
    S06,
    /// The `S07` check.
    S07,
    /// The `S08` check.
    S08,
    /// The `S09` check.
    S09,
    /// The `S10` check.
    S10,
    /// The `S11` check.
    S11,
    /// The `S12` check.
    S12,
    /// The `T01` check.
    T01,
    /// The `T02` check.
    T02,
    /// The `T03` check.
    T03,
    /// The `T04` check.
    T04,
    /// The `T05` check.
    T05,
    /// The `T06` check.
    T06,
    /// The `P01` check.
    P01,
    /// The `P02` check.
    P02,
    /// The `P03` check.
    P03,
    /// The `P04` check.
    P04,
    /// The `P05` check.
    P05,
    /// The `P06` check.
    P06,
    /// The `P07` check.
    P07,
    /// The `H01` check.
    H01,
    /// The `H02` check.
    H02,
    /// The `H03` check.
    H03,
    /// The `H04` check.
    H04,
    /// The `H05` check.
    H05,
    /// The `H06` check.
    H06,
    /// The `H07` check.
    H07,
    /// The `H08` check.
    H08,
    /// The `H09` check.
    H09,
    /// The `H10` check.
    H10,
    /// The `H11` check.
    H11,
    /// The `H12` check.
    H12,
    /// The `H13` check.
    H13,
    /// The `H14` check.
    H14,
    /// The `H15` check.
    H15,
    /// The `H16` check.
    H16,
    /// The `H17` check.
    H17,
    /// The `H18` check.
    H18,
    /// The `H19` check.
    H19,
    /// The `H20` check.
    H20,
    /// The `H21` check.
    H21,
    /// The `H22` check.
    H22,
    /// The `V01` check.
    V01,
    /// The `V02` check.
    V02,
    /// The `V03` check.
    V03,
    /// The `A01` check.
    A01,
    /// The `A02` check.
    A02,
    /// The `A03` check.
    A03,
    /// The `A04` check.
    A04,
    /// The `A05` check.
    A05,
    /// The `G01` check.
    G01,
    /// The `G02` check.
    G02,
}

impl CheckId {
    /// The number of check IDs.
    pub const COUNT: usize = 114;

    /// Every ID, in registry order.
    pub const ALL: [Self; Self::COUNT] = [
        Self::B01,
        Self::B02,
        Self::B03,
        Self::B04,
        Self::B05,
        Self::B06,
        Self::B07,
        Self::B08,
        Self::B09,
        Self::B10,
        Self::B11,
        Self::B12,
        Self::B13,
        Self::B14,
        Self::B15,
        Self::B16,
        Self::B17,
        Self::B18,
        Self::B19,
        Self::B20,
        Self::B21,
        Self::B22,
        Self::B23,
        Self::B24,
        Self::B25,
        Self::B26,
        Self::B27,
        Self::B28,
        Self::B29,
        Self::B30,
        Self::B31,
        Self::B32,
        Self::B33,
        Self::I01,
        Self::I02,
        Self::I03,
        Self::I04,
        Self::D01,
        Self::D02,
        Self::D03,
        Self::D04,
        Self::O01,
        Self::O02,
        Self::O03,
        Self::O04,
        Self::O05,
        Self::O06,
        Self::O07,
        Self::O08,
        Self::O09,
        Self::O10,
        Self::O11,
        Self::L01,
        Self::L02,
        Self::L03,
        Self::L04,
        Self::L05,
        Self::S01,
        Self::S02,
        Self::S03,
        Self::S04,
        Self::S05,
        Self::S06,
        Self::S07,
        Self::S08,
        Self::S09,
        Self::S10,
        Self::S11,
        Self::S12,
        Self::T01,
        Self::T02,
        Self::T03,
        Self::T04,
        Self::T05,
        Self::T06,
        Self::P01,
        Self::P02,
        Self::P03,
        Self::P04,
        Self::P05,
        Self::P06,
        Self::P07,
        Self::H01,
        Self::H02,
        Self::H03,
        Self::H04,
        Self::H05,
        Self::H06,
        Self::H07,
        Self::H08,
        Self::H09,
        Self::H10,
        Self::H11,
        Self::H12,
        Self::H13,
        Self::H14,
        Self::H15,
        Self::H16,
        Self::H17,
        Self::H18,
        Self::H19,
        Self::H20,
        Self::H21,
        Self::H22,
        Self::V01,
        Self::V02,
        Self::V03,
        Self::A01,
        Self::A02,
        Self::A03,
        Self::A04,
        Self::A05,
        Self::G01,
        Self::G02,
    ];

    /// The ID as it is written in reports and configs, for example `"B01"`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::B01 => "B01",
            Self::B02 => "B02",
            Self::B03 => "B03",
            Self::B04 => "B04",
            Self::B05 => "B05",
            Self::B06 => "B06",
            Self::B07 => "B07",
            Self::B08 => "B08",
            Self::B09 => "B09",
            Self::B10 => "B10",
            Self::B11 => "B11",
            Self::B12 => "B12",
            Self::B13 => "B13",
            Self::B14 => "B14",
            Self::B15 => "B15",
            Self::B16 => "B16",
            Self::B17 => "B17",
            Self::B18 => "B18",
            Self::B19 => "B19",
            Self::B20 => "B20",
            Self::B21 => "B21",
            Self::B22 => "B22",
            Self::B23 => "B23",
            Self::B24 => "B24",
            Self::B25 => "B25",
            Self::B26 => "B26",
            Self::B27 => "B27",
            Self::B28 => "B28",
            Self::B29 => "B29",
            Self::B30 => "B30",
            Self::B31 => "B31",
            Self::B32 => "B32",
            Self::B33 => "B33",
            Self::I01 => "I01",
            Self::I02 => "I02",
            Self::I03 => "I03",
            Self::I04 => "I04",
            Self::D01 => "D01",
            Self::D02 => "D02",
            Self::D03 => "D03",
            Self::D04 => "D04",
            Self::O01 => "O01",
            Self::O02 => "O02",
            Self::O03 => "O03",
            Self::O04 => "O04",
            Self::O05 => "O05",
            Self::O06 => "O06",
            Self::O07 => "O07",
            Self::O08 => "O08",
            Self::O09 => "O09",
            Self::O10 => "O10",
            Self::O11 => "O11",
            Self::L01 => "L01",
            Self::L02 => "L02",
            Self::L03 => "L03",
            Self::L04 => "L04",
            Self::L05 => "L05",
            Self::S01 => "S01",
            Self::S02 => "S02",
            Self::S03 => "S03",
            Self::S04 => "S04",
            Self::S05 => "S05",
            Self::S06 => "S06",
            Self::S07 => "S07",
            Self::S08 => "S08",
            Self::S09 => "S09",
            Self::S10 => "S10",
            Self::S11 => "S11",
            Self::S12 => "S12",
            Self::T01 => "T01",
            Self::T02 => "T02",
            Self::T03 => "T03",
            Self::T04 => "T04",
            Self::T05 => "T05",
            Self::T06 => "T06",
            Self::P01 => "P01",
            Self::P02 => "P02",
            Self::P03 => "P03",
            Self::P04 => "P04",
            Self::P05 => "P05",
            Self::P06 => "P06",
            Self::P07 => "P07",
            Self::H01 => "H01",
            Self::H02 => "H02",
            Self::H03 => "H03",
            Self::H04 => "H04",
            Self::H05 => "H05",
            Self::H06 => "H06",
            Self::H07 => "H07",
            Self::H08 => "H08",
            Self::H09 => "H09",
            Self::H10 => "H10",
            Self::H11 => "H11",
            Self::H12 => "H12",
            Self::H13 => "H13",
            Self::H14 => "H14",
            Self::H15 => "H15",
            Self::H16 => "H16",
            Self::H17 => "H17",
            Self::H18 => "H18",
            Self::H19 => "H19",
            Self::H20 => "H20",
            Self::H21 => "H21",
            Self::H22 => "H22",
            Self::V01 => "V01",
            Self::V02 => "V02",
            Self::V03 => "V03",
            Self::A01 => "A01",
            Self::A02 => "A02",
            Self::A03 => "A03",
            Self::A04 => "A04",
            Self::A05 => "A05",
            Self::G01 => "G01",
            Self::G02 => "G02",
        }
    }

    /// The ID's prefix letter, and so its family.
    #[must_use]
    pub const fn family(self) -> Family {
        match self {
            Self::B01 => Family::B,
            Self::B02 => Family::B,
            Self::B03 => Family::B,
            Self::B04 => Family::B,
            Self::B05 => Family::B,
            Self::B06 => Family::B,
            Self::B07 => Family::B,
            Self::B08 => Family::B,
            Self::B09 => Family::B,
            Self::B10 => Family::B,
            Self::B11 => Family::B,
            Self::B12 => Family::B,
            Self::B13 => Family::B,
            Self::B14 => Family::B,
            Self::B15 => Family::B,
            Self::B16 => Family::B,
            Self::B17 => Family::B,
            Self::B18 => Family::B,
            Self::B19 => Family::B,
            Self::B20 => Family::B,
            Self::B21 => Family::B,
            Self::B22 => Family::B,
            Self::B23 => Family::B,
            Self::B24 => Family::B,
            Self::B25 => Family::B,
            Self::B26 => Family::B,
            Self::B27 => Family::B,
            Self::B28 => Family::B,
            Self::B29 => Family::B,
            Self::B30 => Family::B,
            Self::B31 => Family::B,
            Self::B32 => Family::B,
            Self::B33 => Family::B,
            Self::I01 => Family::I,
            Self::I02 => Family::I,
            Self::I03 => Family::I,
            Self::I04 => Family::I,
            Self::D01 => Family::D,
            Self::D02 => Family::D,
            Self::D03 => Family::D,
            Self::D04 => Family::D,
            Self::O01 => Family::O,
            Self::O02 => Family::O,
            Self::O03 => Family::O,
            Self::O04 => Family::O,
            Self::O05 => Family::O,
            Self::O06 => Family::O,
            Self::O07 => Family::O,
            Self::O08 => Family::O,
            Self::O09 => Family::O,
            Self::O10 => Family::O,
            Self::O11 => Family::O,
            Self::L01 => Family::L,
            Self::L02 => Family::L,
            Self::L03 => Family::L,
            Self::L04 => Family::L,
            Self::L05 => Family::L,
            Self::S01 => Family::S,
            Self::S02 => Family::S,
            Self::S03 => Family::S,
            Self::S04 => Family::S,
            Self::S05 => Family::S,
            Self::S06 => Family::S,
            Self::S07 => Family::S,
            Self::S08 => Family::S,
            Self::S09 => Family::S,
            Self::S10 => Family::S,
            Self::S11 => Family::S,
            Self::S12 => Family::S,
            Self::T01 => Family::T,
            Self::T02 => Family::T,
            Self::T03 => Family::T,
            Self::T04 => Family::T,
            Self::T05 => Family::T,
            Self::T06 => Family::T,
            Self::P01 => Family::P,
            Self::P02 => Family::P,
            Self::P03 => Family::P,
            Self::P04 => Family::P,
            Self::P05 => Family::P,
            Self::P06 => Family::P,
            Self::P07 => Family::P,
            Self::H01 => Family::H,
            Self::H02 => Family::H,
            Self::H03 => Family::H,
            Self::H04 => Family::H,
            Self::H05 => Family::H,
            Self::H06 => Family::H,
            Self::H07 => Family::H,
            Self::H08 => Family::H,
            Self::H09 => Family::H,
            Self::H10 => Family::H,
            Self::H11 => Family::H,
            Self::H12 => Family::H,
            Self::H13 => Family::H,
            Self::H14 => Family::H,
            Self::H15 => Family::H,
            Self::H16 => Family::H,
            Self::H17 => Family::H,
            Self::H18 => Family::H,
            Self::H19 => Family::H,
            Self::H20 => Family::H,
            Self::H21 => Family::H,
            Self::H22 => Family::H,
            Self::V01 => Family::V,
            Self::V02 => Family::V,
            Self::V03 => Family::V,
            Self::A01 => Family::A,
            Self::A02 => Family::A,
            Self::A03 => Family::A,
            Self::A04 => Family::A,
            Self::A05 => Family::A,
            Self::G01 => Family::G,
            Self::G02 => Family::G,
        }
    }

    /// Parses an ID such as `"B01"`.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "B01" => Some(Self::B01),
            "B02" => Some(Self::B02),
            "B03" => Some(Self::B03),
            "B04" => Some(Self::B04),
            "B05" => Some(Self::B05),
            "B06" => Some(Self::B06),
            "B07" => Some(Self::B07),
            "B08" => Some(Self::B08),
            "B09" => Some(Self::B09),
            "B10" => Some(Self::B10),
            "B11" => Some(Self::B11),
            "B12" => Some(Self::B12),
            "B13" => Some(Self::B13),
            "B14" => Some(Self::B14),
            "B15" => Some(Self::B15),
            "B16" => Some(Self::B16),
            "B17" => Some(Self::B17),
            "B18" => Some(Self::B18),
            "B19" => Some(Self::B19),
            "B20" => Some(Self::B20),
            "B21" => Some(Self::B21),
            "B22" => Some(Self::B22),
            "B23" => Some(Self::B23),
            "B24" => Some(Self::B24),
            "B25" => Some(Self::B25),
            "B26" => Some(Self::B26),
            "B27" => Some(Self::B27),
            "B28" => Some(Self::B28),
            "B29" => Some(Self::B29),
            "B30" => Some(Self::B30),
            "B31" => Some(Self::B31),
            "B32" => Some(Self::B32),
            "B33" => Some(Self::B33),
            "I01" => Some(Self::I01),
            "I02" => Some(Self::I02),
            "I03" => Some(Self::I03),
            "I04" => Some(Self::I04),
            "D01" => Some(Self::D01),
            "D02" => Some(Self::D02),
            "D03" => Some(Self::D03),
            "D04" => Some(Self::D04),
            "O01" => Some(Self::O01),
            "O02" => Some(Self::O02),
            "O03" => Some(Self::O03),
            "O04" => Some(Self::O04),
            "O05" => Some(Self::O05),
            "O06" => Some(Self::O06),
            "O07" => Some(Self::O07),
            "O08" => Some(Self::O08),
            "O09" => Some(Self::O09),
            "O10" => Some(Self::O10),
            "O11" => Some(Self::O11),
            "L01" => Some(Self::L01),
            "L02" => Some(Self::L02),
            "L03" => Some(Self::L03),
            "L04" => Some(Self::L04),
            "L05" => Some(Self::L05),
            "S01" => Some(Self::S01),
            "S02" => Some(Self::S02),
            "S03" => Some(Self::S03),
            "S04" => Some(Self::S04),
            "S05" => Some(Self::S05),
            "S06" => Some(Self::S06),
            "S07" => Some(Self::S07),
            "S08" => Some(Self::S08),
            "S09" => Some(Self::S09),
            "S10" => Some(Self::S10),
            "S11" => Some(Self::S11),
            "S12" => Some(Self::S12),
            "T01" => Some(Self::T01),
            "T02" => Some(Self::T02),
            "T03" => Some(Self::T03),
            "T04" => Some(Self::T04),
            "T05" => Some(Self::T05),
            "T06" => Some(Self::T06),
            "P01" => Some(Self::P01),
            "P02" => Some(Self::P02),
            "P03" => Some(Self::P03),
            "P04" => Some(Self::P04),
            "P05" => Some(Self::P05),
            "P06" => Some(Self::P06),
            "P07" => Some(Self::P07),
            "H01" => Some(Self::H01),
            "H02" => Some(Self::H02),
            "H03" => Some(Self::H03),
            "H04" => Some(Self::H04),
            "H05" => Some(Self::H05),
            "H06" => Some(Self::H06),
            "H07" => Some(Self::H07),
            "H08" => Some(Self::H08),
            "H09" => Some(Self::H09),
            "H10" => Some(Self::H10),
            "H11" => Some(Self::H11),
            "H12" => Some(Self::H12),
            "H13" => Some(Self::H13),
            "H14" => Some(Self::H14),
            "H15" => Some(Self::H15),
            "H16" => Some(Self::H16),
            "H17" => Some(Self::H17),
            "H18" => Some(Self::H18),
            "H19" => Some(Self::H19),
            "H20" => Some(Self::H20),
            "H21" => Some(Self::H21),
            "H22" => Some(Self::H22),
            "V01" => Some(Self::V01),
            "V02" => Some(Self::V02),
            "V03" => Some(Self::V03),
            "A01" => Some(Self::A01),
            "A02" => Some(Self::A02),
            "A03" => Some(Self::A03),
            "A04" => Some(Self::A04),
            "A05" => Some(Self::A05),
            "G01" => Some(Self::G01),
            "G02" => Some(Self::G02),
            _ => None,
        }
    }
}

impl fmt::Display for CheckId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A check family, named by the ID prefix letter.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema, TS,
)]
pub enum Family {
    /// Behavioral checks (B).
    #[serde(rename = "B")]
    B,
    /// API checks (I).
    #[serde(rename = "I")]
    I,
    /// Data-model checks (D).
    #[serde(rename = "D")]
    D,
    /// Operability checks (O).
    #[serde(rename = "O")]
    O,
    /// AI-feature checks (L).
    #[serde(rename = "L")]
    L,
    /// Static checks (S).
    #[serde(rename = "S")]
    S,
    /// Test checks (T).
    #[serde(rename = "T")]
    T,
    /// Dependency checks (P).
    #[serde(rename = "P")]
    P,
    /// History checks (H).
    #[serde(rename = "H")]
    H,
    /// Provenance checks (V).
    #[serde(rename = "V")]
    V,
    /// Agent checks (A).
    #[serde(rename = "A")]
    A,
    /// Genericness checks (G).
    #[serde(rename = "G")]
    G,
}

impl Family {
    /// The family's prefix letter.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::B => "B",
            Self::I => "I",
            Self::D => "D",
            Self::O => "O",
            Self::L => "L",
            Self::S => "S",
            Self::T => "T",
            Self::P => "P",
            Self::H => "H",
            Self::V => "V",
            Self::A => "A",
            Self::G => "G",
        }
    }

    /// The families that count toward a slop label (ADR-0012).
    #[must_use]
    pub const fn counts_for_slop(self) -> bool {
        match self {
            Self::B
            | Self::I
            | Self::D
            | Self::O
            | Self::L
            | Self::S
            | Self::T
            | Self::P
            | Self::H => true,
            Self::V | Self::A | Self::G => false,
        }
    }
}

/// How bad a failure is. Fixed per check, uncalibrated (ADR-0010).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema, TS,
)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Critical: a hard gate or a severe trust failure.
    Critical,
    /// High.
    High,
    /// Medium.
    Medium,
    /// Low.
    Low,
    /// Informational.
    Info,
}

/// How much a finding can be trusted.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema, TS,
)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// Confirmed by behavior.
    Confirmed,
    /// Seen inconsistently, set by the 3-of-3 rule at runtime.
    Flaky,
    /// A static hint that explains rather than decides.
    Hint,
}

/// How destructive a check is. Recorded, but gates nothing in v1.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema, TS,
)]
pub enum Level {
    /// L0: reads the repository or recorded data only.
    #[serde(rename = "L0")]
    L0,
    /// L1: runs a read-only flow against a disposable working copy.
    #[serde(rename = "L1")]
    L1,
    /// L2: writes through the app, then discards the data.
    #[serde(rename = "L2")]
    L2,
    /// L3: faults, resets, restarts or upgrade runs.
    #[serde(rename = "L3")]
    L3,
}

/// The independent questions a score answers.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema, TS,
)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    /// Works now.
    WorksNow,
    /// Honest.
    Honest,
    /// Robust.
    Robust,
    /// Safe.
    Safe,
    /// Data integrity.
    DataIntegrity,
    /// Operable.
    Operable,
    /// Maintainable.
    Maintainable,
    /// Vitality.
    Vitality,
    /// Provenance; shown as a panel and never scored.
    Provenance,
    /// Generic; later.
    Generic,
}

/// What a check needs before it can run.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema, TS,
)]
#[serde(rename_all = "snake_case")]
pub enum Need {
    /// The repository under test.
    Repo,
    /// Permission to execute repo-controlled code (`--allow-exec`).
    Exec,
    /// The forge client and its token.
    Forge,
    /// Package-registry data.
    Registry,
    /// A browser driven by the TypeScript driver.
    Browser,
    /// A database channel.
    Db,
    /// The egress proxy.
    Egress,
    /// Mailpit.
    Mail,
    /// Toxiproxy.
    Faults,
    /// A controllable server clock.
    Clock,
    /// An OpenAPI spec, shipped or inferred.
    Spec,
}

/// The milestone that delivers a check.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema, TS,
)]
pub enum Milestone {
    /// M1.
    #[serde(rename = "M1")]
    M1,
    /// M2.
    #[serde(rename = "M2")]
    M2,
    /// M3.
    #[serde(rename = "M3")]
    M3,
    /// M4.
    #[serde(rename = "M4")]
    M4,
    /// M5.
    #[serde(rename = "M5")]
    M5,
    /// M6.
    #[serde(rename = "M6")]
    M6,
    /// M7.
    #[serde(rename = "M7")]
    M7,
    /// M8.
    #[serde(rename = "M8")]
    M8,
    /// M9.
    #[serde(rename = "M9")]
    M9,
    /// M10.
    #[serde(rename = "M10")]
    M10,
    /// M11.
    #[serde(rename = "M11")]
    M11,
    /// M12.
    #[serde(rename = "M12")]
    M12,
    /// M13.
    #[serde(rename = "M13")]
    M13,
    /// Later, or out of v1 scope.
    #[serde(rename = "later")]
    Later,
}

impl Milestone {
    /// The milestone as the plan spells it: `M1` … `M13`, or `later`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::M1 => "M1",
            Self::M2 => "M2",
            Self::M3 => "M3",
            Self::M4 => "M4",
            Self::M5 => "M5",
            Self::M6 => "M6",
            Self::M7 => "M7",
            Self::M8 => "M8",
            Self::M9 => "M9",
            Self::M10 => "M10",
            Self::M11 => "M11",
            Self::M12 => "M12",
            Self::M13 => "M13",
            Self::Later => "later",
        }
    }
}

/// How far a check has been built.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema, TS,
)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    /// Registered; its task has not landed.
    Planned,
    /// Registered and delivered.
    Implemented,
    /// The ID is reserved for later; it never runs.
    Reserved,
    /// A stage that supports other checks rather than judging.
    Support,
}

/// When a check's failure fails the whole run, regardless of other results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateRule {
    /// Never a gate.
    None,
    /// Any failure is a gate failure.
    Always,
    /// Only the named sub-checks gate.
    SubChecks(&'static [&'static str]),
}

impl GateRule {
    /// The named sub-checks, empty when the rule names none.
    #[must_use]
    pub fn sub_checks(&self) -> &'static [&'static str] {
        match self {
            Self::SubChecks(names) => names,
            Self::None | Self::Always => &[],
        }
    }
}

impl Serialize for GateRule {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;

        let mut state = serializer.serialize_struct("GateRule", 2)?;
        match self {
            Self::None => {
                state.serialize_field("kind", "none")?;
                state.serialize_field("checks", &[] as &[&str])?;
            }
            Self::Always => {
                state.serialize_field("kind", "always")?;
                state.serialize_field("checks", &[] as &[&str])?;
            }
            Self::SubChecks(names) => {
                state.serialize_field("kind", "sub_checks")?;
                state.serialize_field("checks", names)?;
            }
        }
        state.end()
    }
}

/// The positional discriminants must fit a `u8`, since the enum is `#[repr(u8)]`.
const _: () = assert!(CheckId::COUNT <= u8::MAX as usize);
const _: () = assert!(CheckId::ALL.len() == CheckId::COUNT);
