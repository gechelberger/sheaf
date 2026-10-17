//! Adobe's Core 14 font metrics (`assets/afm`; `assets/LICENSE-afm` has
//! Adobe's notice): each glyph's advance, and the code the font's own
//! encoding gives it. The Latin fonts' own encoding is StandardEncoding;
//! Symbol's and ZapfDingbats' are their own.

use std::collections::HashMap;

/// One font's metrics.
#[derive(Debug)]
pub(super) struct Afm {
    /// Each glyph's advance, per 1000 em, by name.
    widths: HashMap<&'static str, u16>,
    /// The font's own encoding: each code's glyph.
    codes: [Option<&'static str>; 256],
}

impl Afm {
    /// The metrics in `text`, an AFM file: its `C … ; WX … ; N … ;` lines.
    /// A line without a name or a whole width is passed over.
    pub fn parse(text: &'static str) -> Afm {
        let mut afm = Afm {
            widths: HashMap::new(),
            codes: [None; 256],
        };
        let metrics = text
            .lines()
            .skip_while(|l| !l.starts_with("StartCharMetrics"))
            .skip(1)
            .take_while(|l| !l.starts_with("EndCharMetrics"));
        for line in metrics {
            let (mut code, mut width, mut name) = (None, None, None);
            for field in line.split(';') {
                let mut words = field.split_whitespace();
                match (words.next(), words.next()) {
                    (Some("C"), Some(c)) => code = c.parse::<i32>().ok(),
                    (Some("WX"), Some(w)) => width = w.parse::<u16>().ok(),
                    (Some("N"), Some(n)) => name = Some(n),
                    _ => {}
                }
            }
            let (Some(name), Some(width)) = (name, width) else {
                continue;
            };
            afm.widths.insert(name, width);
            if let Some(code) = code.and_then(|c| u8::try_from(c).ok()) {
                afm.codes[code as usize] = Some(name);
            }
        }
        afm
    }

    /// The advance of glyph `name`, per 1000 em; None for a glyph the
    /// font hasn't.
    pub fn width(&self, name: &str) -> Option<u16> {
        self.widths.get(name).copied()
    }

    /// The glyph the font's own encoding gives `code`.
    pub fn glyph(&self, code: u8) -> Option<&'static str> {
        self.codes[code as usize]
    }

    /// How many glyphs it has.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.widths.len()
    }
}
