#[derive(Debug, Clone, Copy)]
pub struct Rgb {
    pub(crate) r: u8,
    pub(crate) g: u8,
    pub(crate) b: u8,
}

#[derive(Debug, Clone, Copy)]
pub struct Ansi {
    pub(crate) code: u8,
    pub(crate) named: bool,
}

#[derive(serde::Deserialize, serde::Serialize, Debug, Clone, Copy)]
#[serde(untagged)]
pub enum Color {
    Rgb(Rgb),
    Ansi(Ansi),
}

impl Color {
    /// Convert Self::Ansi into Self::Rgb
    pub fn into_rgb(self) -> Rgb {
        match self {
            Self::Rgb(rgb) => rgb,
            Self::Ansi(ansi) => ansi.into_rgb(),
        }
    }
    pub fn dim(self, frac: f32) -> Self {
        let rgb = self.into_rgb();
        Self::Rgb(Rgb {
            r: (rgb.r as f32 * frac) as u8,
            g: (rgb.g as f32 * frac) as u8,
            b: (rgb.b as f32 * frac) as u8,
        })
    }
    pub fn invert(self) -> Self {
        let rgb = self.into_rgb();
        Self::Rgb(Rgb {
            r: 255 - rgb.r,
            g: 255 - rgb.g,
            b: 255 - rgb.b,
        })
    }
}

impl From<Color> for anstyle::Color {
    fn from(value: Color) -> Self {
        match value {
            Color::Rgb(rgb) => anstyle::Color::Rgb(anstyle::RgbColor(rgb.r, rgb.g, rgb.b)),
            Color::Ansi(ansi) => anstyle::Color::Ansi256(anstyle::Ansi256Color(ansi.code)),
        }
    }
}

impl From<anstyle::Color> for Color {
    fn from(value: anstyle::Color) -> Self {
        match value {
            anstyle::Color::Ansi(ansi) => Self::Ansi(Ansi {
                code: ansi as u8,
                named: true,
            }),
            anstyle::Color::Ansi256(ansi256) => Self::Ansi(Ansi {
                code: ansi256.0,
                named: false,
            }),
            anstyle::Color::Rgb(rgb) => Self::Rgb(Rgb {
                r: rgb.r(),
                g: rgb.g(),
                b: rgb.b(),
            }),
        }
    }
}

impl<'de> serde::Deserialize<'de> for Rgb {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        try {
            let s = <String as serde::Deserialize<'_>>::deserialize(deserializer);

            let s = s?;
            let s = s
                .strip_prefix('#')
                .ok_or_else(|| serde::de::Error::custom(format!("invalid rgb {s}")))?;
            if s.len() != 6 {
                return Err(serde::de::Error::custom(format!("invalid rgb {s}")));
            }

            let r = u8::from_str_radix(&s[..2], 16).map_err(serde::de::Error::custom)?;
            let g = u8::from_str_radix(&s[2..4], 16).map_err(serde::de::Error::custom)?;
            let b = u8::from_str_radix(&s[4..], 16).map_err(serde::de::Error::custom)?;

            Rgb { r, g, b }
        }
    }
}

impl serde::Serialize for Rgb {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b))
    }
}

const ANSI_NAMES: &[&str] = &[
    "black", "red", "green", "yellow", "blue", "purple", "cyan", "white",
];

const BASE_6_COLOR_RAMP: &[u8] = &[0, 95, 135, 175, 215, 255];

impl Ansi {
    pub fn into_rgb(self) -> Rgb {
        if self.code < 16 {
            let base = if self.code > 8 {
                self.code - 8
            } else {
                self.code
            };
            let intensity = if self.code > 8 { 255 } else { 205 };
            Rgb {
                r: (base & 1) * intensity,
                g: (base & 2) / 2 * intensity,
                b: (base & 4) / 4 * intensity,
            }
        } else if self.code < 232 {
            // base-6 encoded rgb
            let base = self.code - 16;
            Rgb {
                b: BASE_6_COLOR_RAMP[(base % 6) as usize],
                g: BASE_6_COLOR_RAMP[(base / 6 % 6) as usize],
                r: BASE_6_COLOR_RAMP[(base / 36) as usize],
            }
        } else {
            // grey scale
            let base = self.code - 232;
            Rgb {
                r: 8 + base * 10,
                g: 8 + base * 10,
                b: 8 + base * 10,
            }
        }
    }
}

impl<'de> serde::Deserialize<'de> for Ansi {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = <String as serde::Deserialize<'_>>::deserialize(deserializer)?;

        Ok(if let Ok(code) = s.parse::<u8>() {
            Ansi { code, named: false }
        } else {
            let (intense, s) = s
                .strip_prefix("bright")
                .map(|s| (true, s))
                .unwrap_or((false, &s));
            let intense = if intense { 8 } else { 0 };
            Ansi {
                code: ANSI_NAMES
                    .iter()
                    .position(|&e| e == s)
                    .ok_or_else(|| serde::de::Error::custom(format!("invalid color {s}")))?
                    as u8
                    + intense,
                named: true,
            }
        })
    }
}

impl serde::Serialize for Ansi {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        if self.named {
            serializer.serialize_str(&format!(
                "{}{}",
                if self.code > 7 { "bright" } else { "" },
                ANSI_NAMES[(self.code & 7) as usize]
            ))
        } else {
            serializer.serialize_u8(self.code)
        }
    }
}
