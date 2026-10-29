//! QR codes (ISO/IEC 18004), byte mode, versions 1 to 40, all four error correction levels: the
//! smallest version that holds the text, Reed–Solomon error correction over GF(256), the blocks
//! interleaved, and the mask with the lowest penalty. Written from the standard's description; the
//! tests read each code back with a reader of their own.

/// Error correction level: how much of the code may be damaged and still read (about 7, 15, 25
/// and 30 per cent).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ecc {
    Low,
    Medium,
    Quartile,
    High,
}

impl Ecc {
    fn ordinal(self) -> usize {
        self as usize
    }

    /// The two bits the format information uses.
    fn format_bits(self) -> u32 {
        match self {
            Ecc::Low => 1,
            Ecc::Medium => 0,
            Ecc::Quartile => 3,
            Ecc::High => 2,
        }
    }
}

/// Error correction codewords per block, by level and version (index 0 unused).
const ECC_CODEWORDS_PER_BLOCK: [[u8; 41]; 4] = [
    [
        0, 7, 10, 15, 20, 26, 18, 20, 24, 30, 18, 20, 24, 26, 30, 22, 24, 28, 30, 28, 28, 28, 28,
        30, 30, 26, 28, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30,
    ],
    [
        0, 10, 16, 26, 18, 24, 16, 18, 22, 22, 26, 30, 22, 22, 24, 24, 28, 28, 26, 26, 26, 26, 28,
        28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28,
    ],
    [
        0, 13, 22, 18, 26, 18, 24, 18, 22, 20, 24, 28, 26, 24, 20, 30, 24, 28, 28, 26, 30, 28, 30,
        30, 30, 30, 28, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30,
    ],
    [
        0, 17, 28, 22, 16, 22, 28, 26, 26, 24, 28, 24, 28, 22, 24, 24, 30, 28, 28, 26, 28, 30, 24,
        30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30,
    ],
];

/// Error correction blocks, by level and version (index 0 unused).
const ERROR_CORRECTION_BLOCKS: [[u8; 41]; 4] = [
    [
        0, 1, 1, 1, 1, 1, 2, 2, 2, 2, 4, 4, 4, 4, 4, 6, 6, 6, 6, 7, 8, 8, 9, 9, 10, 12, 12, 12, 13,
        14, 15, 16, 17, 18, 19, 19, 20, 21, 22, 24, 25,
    ],
    [
        0, 1, 1, 1, 2, 2, 4, 4, 4, 5, 5, 5, 8, 9, 9, 10, 10, 11, 13, 14, 16, 17, 17, 18, 20, 21,
        23, 25, 26, 28, 29, 31, 33, 35, 37, 38, 40, 43, 45, 47, 49,
    ],
    [
        0, 1, 1, 2, 2, 4, 4, 6, 6, 8, 8, 8, 10, 12, 16, 12, 17, 16, 18, 21, 20, 23, 23, 25, 27, 29,
        34, 34, 35, 38, 40, 43, 45, 48, 51, 53, 56, 59, 62, 65, 68,
    ],
    [
        0, 1, 1, 2, 4, 4, 4, 5, 6, 8, 8, 11, 11, 16, 16, 18, 16, 19, 21, 25, 25, 25, 34, 30, 32,
        35, 37, 40, 42, 45, 48, 51, 54, 57, 60, 63, 66, 70, 74, 77, 81,
    ],
];

/// Modules that carry data or error correction in a code of `ver`: everything but the function
/// patterns.
fn raw_data_modules(ver: usize) -> usize {
    let mut result = (16 * ver + 128) * ver + 64;
    if ver >= 2 {
        let align = ver / 7 + 2;
        result -= (25 * align - 10) * align - 55;
        if ver >= 7 {
            result -= 36;
        }
    }
    result
}

fn data_codewords(ver: usize, ecl: Ecc) -> usize {
    raw_data_modules(ver) / 8
        - ECC_CODEWORDS_PER_BLOCK[ecl.ordinal()][ver] as usize
            * ERROR_CORRECTION_BLOCKS[ecl.ordinal()][ver] as usize
}

/// Product in GF(2^8) modulo x^8 + x^4 + x^3 + x^2 + 1.
fn gf_multiply(x: u8, y: u8) -> u8 {
    let mut z: u32 = 0;
    for i in (0..8).rev() {
        z = (z << 1) ^ ((z >> 7) * 0x11D);
        z ^= ((y as u32 >> i) & 1) * x as u32;
    }
    z as u8
}

/// The Reed–Solomon generator polynomial of `degree`, highest power first, its leading 1 left out.
fn rs_divisor(degree: usize) -> Vec<u8> {
    let mut result = vec![0u8; degree - 1];
    result.push(1);
    let mut root: u8 = 1;
    for _ in 0..degree {
        for j in 0..degree {
            result[j] = gf_multiply(result[j], root);
            if j + 1 < degree {
                result[j] ^= result[j + 1];
            }
        }
        root = gf_multiply(root, 0x02);
    }
    result
}

/// The error correction codewords of `data` for the divisor.
fn rs_remainder(data: &[u8], divisor: &[u8]) -> Vec<u8> {
    let mut result = vec![0u8; divisor.len()];
    for &b in data {
        let factor = b ^ result.remove(0);
        result.push(0);
        for (x, &y) in result.iter_mut().zip(divisor) {
            *x ^= gf_multiply(y, factor);
        }
    }
    result
}

/// A QR code's modules: `size` × `size`, true for dark, row by row from the top left.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QrCode {
    pub version: usize,
    pub size: usize,
    pub ecl: Ecc,
    pub mask: u8,
    modules: Vec<bool>,
    function: Vec<bool>,
}

impl QrCode {
    /// The module in column `x` of row `y` (both from the top left).
    pub fn dark(&self, x: usize, y: usize) -> bool {
        self.modules[y * self.size + x]
    }

    fn blank(version: usize, ecl: Ecc) -> QrCode {
        let size = version * 4 + 17;
        QrCode {
            version,
            size,
            ecl,
            mask: 0,
            modules: vec![false; size * size],
            function: vec![false; size * size],
        }
    }

    fn set_function(&mut self, x: usize, y: usize, dark: bool) {
        self.modules[y * self.size + x] = dark;
        self.function[y * self.size + x] = true;
    }

    fn alignment_positions(&self) -> Vec<usize> {
        let ver = self.version;
        if ver == 1 {
            return Vec::new();
        }
        let n = ver / 7 + 2;
        let step = (ver * 8 + n * 3 + 5) / (n * 4 - 4) * 2;
        let mut out: Vec<usize> = (0..n - 1).map(|i| self.size - 7 - i * step).collect();
        out.push(6);
        out.reverse();
        out
    }

    fn draw_function_patterns(&mut self) {
        for i in 0..self.size {
            self.set_function(6, i, i % 2 == 0);
            self.set_function(i, 6, i % 2 == 0);
        }
        let far = self.size - 4;
        for (x, y) in [(3, 3), (far, 3), (3, far)] {
            for dy in -4i32..=4 {
                for dx in -4i32..=4 {
                    let (xx, yy) = (x as i32 + dx, y as i32 + dy);
                    if xx >= 0 && yy >= 0 && (xx as usize) < self.size && (yy as usize) < self.size
                    {
                        let d = dx.abs().max(dy.abs());
                        self.set_function(xx as usize, yy as usize, d != 2 && d != 4);
                    }
                }
            }
        }
        let align = self.alignment_positions();
        let n = align.len();
        for i in 0..n {
            for j in 0..n {
                if (i == 0 && j == 0) || (i == 0 && j == n - 1) || (i == n - 1 && j == 0) {
                    continue;
                }
                for dy in -2i32..=2 {
                    for dx in -2i32..=2 {
                        self.set_function(
                            (align[i] as i32 + dx) as usize,
                            (align[j] as i32 + dy) as usize,
                            dx.abs().max(dy.abs()) != 1,
                        );
                    }
                }
            }
        }
        self.draw_format_bits(0);
        self.draw_version();
    }

    fn draw_format_bits(&mut self, mask: u8) {
        let bits = format_bits(self.ecl, mask);
        let bit = |i: u32| (bits >> i) & 1 != 0;
        for i in 0..=5 {
            self.set_function(8, i as usize, bit(i));
        }
        self.set_function(8, 7, bit(6));
        self.set_function(8, 8, bit(7));
        self.set_function(7, 8, bit(8));
        for i in 9..15 {
            self.set_function(14 - i as usize, 8, bit(i));
        }
        let size = self.size;
        for i in 0..8 {
            self.set_function(size - 1 - i as usize, 8, bit(i));
        }
        for i in 8..15 {
            self.set_function(8, size - 15 + i as usize, bit(i));
        }
        self.set_function(8, size - 8, true);
    }

    fn draw_version(&mut self) {
        if self.version < 7 {
            return;
        }
        let bits = version_bits(self.version);
        for i in 0..18 {
            let bit = (bits >> i) & 1 != 0;
            let a = self.size - 11 + i % 3;
            let b = i / 3;
            self.set_function(a, b, bit);
            self.set_function(b, a, bit);
        }
    }

    /// The data modules in reading order: two columns at a time from the right, up and down in
    /// turn, skipping the vertical timing column.
    fn data_order(&self) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        let mut right = self.size as isize - 1;
        while right >= 1 {
            if right == 6 {
                right = 5;
            }
            for vert in 0..self.size {
                for j in 0..2 {
                    let x = (right - j) as usize;
                    let upward = (right + 1) & 2 == 0;
                    let y = if upward { self.size - 1 - vert } else { vert };
                    if !self.function[y * self.size + x] {
                        out.push((x, y));
                    }
                }
            }
            right -= 2;
        }
        out
    }

    fn draw_codewords(&mut self, data: &[u8]) {
        for (i, (x, y)) in self.data_order().into_iter().enumerate() {
            // Remainder modules (past the last codeword) stay light.
            let dark = i < data.len() * 8 && (data[i >> 3] >> (7 - (i & 7))) & 1 != 0;
            self.modules[y * self.size + x] = dark;
        }
    }

    fn apply_mask(&mut self, mask: u8) {
        for y in 0..self.size {
            for x in 0..self.size {
                if !self.function[y * self.size + x] && masked(mask, x, y) {
                    self.modules[y * self.size + x] ^= true;
                }
            }
        }
    }

    /// The standard's penalty for the code as drawn: long runs, 2 × 2 blocks, finder-like
    /// patterns and an unbalanced share of dark modules.
    fn penalty(&self) -> u32 {
        let n = self.size;
        let at = |x: usize, y: usize| self.modules[y * n + x];
        let mut score = 0u32;
        for across in [true, false] {
            for a in 0..n {
                let get = |b: usize| if across { at(b, a) } else { at(a, b) };
                let mut run = 1;
                for b in 1..=n {
                    if b < n && get(b) == get(b - 1) {
                        run += 1;
                        continue;
                    }
                    if run >= 5 {
                        score += 3 + (run - 5) as u32;
                    }
                    run = 1;
                }
                // 1:1:3:1:1 dark-light-dark-light-dark with four light modules on one side.
                const PATTERN: [bool; 11] = [
                    true, false, true, true, true, false, true, false, false, false, false,
                ];
                for b in 0..=n.saturating_sub(11) {
                    let window: Vec<bool> = (b..b + 11).map(get).collect();
                    if window == PATTERN || window.iter().rev().copied().eq(PATTERN) {
                        score += 40;
                    }
                }
            }
        }
        for y in 0..n - 1 {
            for x in 0..n - 1 {
                let c = at(x, y);
                if c == at(x + 1, y) && c == at(x, y + 1) && c == at(x + 1, y + 1) {
                    score += 3;
                }
            }
        }
        let dark = self.modules.iter().filter(|&&d| d).count();
        let total = n * n;
        // Each whole 5 % step away from half dark costs 10.
        let k = ((dark * 20).abs_diff(total * 10))
            .div_ceil(total)
            .saturating_sub(1);
        score + 10 * k as u32
    }
}

fn masked(mask: u8, x: usize, y: usize) -> bool {
    match mask {
        0 => (x + y).is_multiple_of(2),
        1 => y.is_multiple_of(2),
        2 => x.is_multiple_of(3),
        3 => (x + y).is_multiple_of(3),
        4 => (x / 3 + y / 2).is_multiple_of(2),
        5 => x * y % 2 + x * y % 3 == 0,
        6 => (x * y % 2 + x * y % 3).is_multiple_of(2),
        _ => ((x + y) % 2 + x * y % 3).is_multiple_of(2),
    }
}

/// The 15 format bits for a level and mask: 5 data bits, 10 BCH bits, then the fixed XOR mask.
fn format_bits(ecl: Ecc, mask: u8) -> u32 {
    let data = ecl.format_bits() << 3 | mask as u32;
    let mut rem = data;
    for _ in 0..10 {
        rem = (rem << 1) ^ ((rem >> 9) * 0x537);
    }
    (data << 10 | rem) ^ 0x5412
}

/// The 18 version bits (versions 7 and up): 6 data bits and 12 BCH bits.
fn version_bits(version: usize) -> u32 {
    let mut rem = version as u32;
    for _ in 0..12 {
        rem = (rem << 1) ^ ((rem >> 11) * 0x1F25);
    }
    (version as u32) << 12 | rem
}

/// Splits the data codewords into blocks, adds each block's error correction, and interleaves
/// them codeword by codeword.
fn add_ecc_and_interleave(data: &[u8], version: usize, ecl: Ecc) -> Vec<u8> {
    let blocks = ERROR_CORRECTION_BLOCKS[ecl.ordinal()][version] as usize;
    let ecc_len = ECC_CODEWORDS_PER_BLOCK[ecl.ordinal()][version] as usize;
    let raw = raw_data_modules(version) / 8;
    let short_blocks = blocks - raw % blocks;
    let short_len = raw / blocks;
    let divisor = rs_divisor(ecc_len);
    let mut all: Vec<Vec<u8>> = Vec::with_capacity(blocks);
    let mut k = 0;
    for i in 0..blocks {
        let len = short_len - ecc_len + usize::from(i >= short_blocks);
        let dat = &data[k..k + len];
        k += len;
        let mut block = dat.to_vec();
        if i < short_blocks {
            // A placeholder so every block has the same length while interleaving.
            block.push(0);
        }
        block.extend(rs_remainder(dat, &divisor));
        all.push(block);
    }
    let mut out = Vec::with_capacity(raw);
    for i in 0..all[0].len() {
        for (j, block) in all.iter().enumerate() {
            if i != short_len - ecc_len || j >= short_blocks {
                out.push(block[i]);
            }
        }
    }
    out
}

/// The QR code of `text` (as UTF-8 bytes) at error correction level `ecl`, the smallest version
/// that holds it. Fails with a sentence when the text is too long for any version.
pub fn encode(text: &str, ecl: Ecc) -> Result<QrCode, String> {
    let data = text.as_bytes();
    let count_bits = |ver: usize| if ver <= 9 { 8 } else { 16 };
    let version = (1..=40)
        .find(|&v| 4 + count_bits(v) + 8 * data.len() <= data_codewords(v, ecl) * 8)
        .ok_or_else(|| {
            format!(
                "The text is too long for a QR code at this error correction level ({} bytes; at most {}).",
                data.len(),
                data_codewords(40, ecl) - 3
            )
        })?;
    // Mode, count, data, terminator, padding to whole bytes, then the pad bytes.
    let capacity = data_codewords(version, ecl) * 8;
    let mut bits: Vec<bool> = Vec::with_capacity(capacity);
    let mut push = |value: u32, len: usize| {
        for i in (0..len).rev() {
            bits.push((value >> i) & 1 != 0);
        }
    };
    push(0b0100, 4);
    push(data.len() as u32, count_bits(version));
    for &b in data {
        push(b as u32, 8);
    }
    let terminator = (capacity - bits.len()).min(4);
    bits.extend(std::iter::repeat_n(false, terminator));
    bits.extend(std::iter::repeat_n(false, (8 - bits.len() % 8) % 8));
    let mut codewords: Vec<u8> = bits
        .chunks(8)
        .map(|c| c.iter().fold(0u8, |a, &b| a << 1 | u8::from(b)))
        .collect();
    for pad in [0xEC, 0x11].into_iter().cycle() {
        if codewords.len() >= capacity / 8 {
            break;
        }
        codewords.push(pad);
    }
    let all = add_ecc_and_interleave(&codewords, version, ecl);
    let mut code = QrCode::blank(version, ecl);
    code.draw_function_patterns();
    code.draw_codewords(&all);
    let mut best: Option<(u32, u8)> = None;
    for mask in 0..8u8 {
        code.apply_mask(mask);
        code.draw_format_bits(mask);
        let p = code.penalty();
        if best.is_none_or(|(b, _)| p < b) {
            best = Some((p, mask));
        }
        code.apply_mask(mask);
    }
    let mask = best.expect("eight masks were tried").1;
    code.apply_mask(mask);
    code.draw_format_bits(mask);
    code.mask = mask;
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reed_solomon_matches_the_standards_worked_examples() {
        // ISO/IEC 18004 Annex I: "01234567" at 1-M.
        let data = [
            0x10, 0x20, 0x0C, 0x56, 0x61, 0x80, 0xEC, 0x11, 0xEC, 0x11, 0xEC, 0x11, 0xEC, 0x11,
            0xEC, 0x11,
        ];
        assert_eq!(
            rs_remainder(&data, &rs_divisor(10)),
            [0xA5, 0x24, 0xD4, 0xC1, 0xED, 0x36, 0xC7, 0x87, 0x2C, 0x55]
        );
        // "HELLO WORLD" at 1-Q.
        let data = [32, 91, 11, 120, 209, 114, 220, 77, 67, 64, 236, 17, 236];
        assert_eq!(
            rs_remainder(&data, &rs_divisor(13)),
            [168, 72, 22, 82, 217, 54, 156, 0, 46, 15, 180, 122, 16]
        );
    }

    #[test]
    fn capacities_and_bch_codes_match_the_standard() {
        // Data codewords of the smallest and largest versions at each level.
        assert_eq!(
            [Ecc::Low, Ecc::Medium, Ecc::Quartile, Ecc::High].map(|e| data_codewords(1, e)),
            [19, 16, 13, 9]
        );
        assert_eq!(
            [Ecc::Low, Ecc::Medium, Ecc::Quartile, Ecc::High].map(|e| data_codewords(40, e)),
            [2956, 2334, 1666, 1276]
        );
        // Every version's codewords split into its blocks exactly.
        for ecl in [Ecc::Low, Ecc::Medium, Ecc::Quartile, Ecc::High] {
            for v in 1..=40 {
                let raw = raw_data_modules(v) / 8;
                let blocks = ERROR_CORRECTION_BLOCKS[ecl.ordinal()][v] as usize;
                let ecc = ECC_CODEWORDS_PER_BLOCK[ecl.ordinal()][v] as usize;
                assert!(raw / blocks > ecc, "{ecl:?} {v}");
            }
        }
        assert_eq!(format_bits(Ecc::Medium, 0), 0b101010000010010);
        assert_eq!(format_bits(Ecc::Low, 0), 0b111011111000100);
        assert_eq!(version_bits(7), 0b000111110010010100);
        assert_eq!(version_bits(40), 0b101000110001101001);
    }

    /// A reader written from the standard: the format information, the data modules in reading
    /// order without the mask, the blocks taken apart, each block's error correction checked, and
    /// the byte-mode text.
    fn read(code: &QrCode) -> (Ecc, String) {
        let n = code.size;
        let mut bits = 0u32;
        let first = [
            (8, 0),
            (8, 1),
            (8, 2),
            (8, 3),
            (8, 4),
            (8, 5),
            (8, 7),
            (8, 8),
            (7, 8),
            (5, 8),
            (4, 8),
            (3, 8),
            (2, 8),
            (1, 8),
            (0, 8),
        ];
        for (i, &(x, y)) in first.iter().enumerate() {
            bits |= u32::from(code.dark(x, y)) << i;
        }
        let (ecl, mask) = [Ecc::Low, Ecc::Medium, Ecc::Quartile, Ecc::High]
            .into_iter()
            .flat_map(|e| (0..8u8).map(move |m| (e, m)))
            .find(|&(e, m)| format_bits(e, m) == bits)
            .expect("valid format information");
        let version = (n - 17) / 4;
        let mut template = QrCode::blank(version, ecl);
        template.draw_function_patterns();
        let mut stream = Vec::new();
        for (x, y) in template.data_order() {
            stream.push(code.dark(x, y) ^ masked(mask, x, y));
        }
        let raw = raw_data_modules(version) / 8;
        let words: Vec<u8> = stream[..raw * 8]
            .chunks(8)
            .map(|c| c.iter().fold(0u8, |a, &b| a << 1 | u8::from(b)))
            .collect();
        let blocks = ERROR_CORRECTION_BLOCKS[ecl.ordinal()][version] as usize;
        let ecc_len = ECC_CODEWORDS_PER_BLOCK[ecl.ordinal()][version] as usize;
        let short_blocks = blocks - raw % blocks;
        let short_len = raw / blocks;
        let lens: Vec<usize> = (0..blocks)
            .map(|i| short_len + usize::from(i >= short_blocks))
            .collect();
        // The inverse of the interleaving: every block padded to the long length, short blocks
        // with an empty slot right after their data.
        let long = short_len + 1;
        let hole = short_len - ecc_len;
        let mut padded = vec![vec![0u8; long]; blocks];
        let mut k = 0;
        for i in 0..long {
            for (j, block) in padded.iter_mut().enumerate() {
                if i == hole && j < short_blocks {
                    continue;
                }
                block[i] = words[k];
                k += 1;
            }
        }
        assert_eq!(k, raw, "every codeword belongs to a block");
        let divisor = rs_divisor(ecc_len);
        let mut data = Vec::new();
        for (j, mut block) in padded.into_iter().enumerate() {
            if j < short_blocks {
                block.remove(hole);
            }
            let (d, e) = block.split_at(lens[j] - ecc_len);
            assert_eq!(
                rs_remainder(d, &divisor),
                e.to_vec(),
                "block {j} is corrupt"
            );
            data.extend_from_slice(d);
        }
        let bit = |i: usize| (data[i / 8] >> (7 - i % 8)) & 1;
        let field = |from: usize, len: usize| {
            (from..from + len).fold(0usize, |a, i| a << 1 | bit(i) as usize)
        };
        assert_eq!(field(0, 4), 0b0100, "byte mode");
        let count_len = if version <= 9 { 8 } else { 16 };
        let count = field(4, count_len);
        let bytes: Vec<u8> = (0..count)
            .map(|i| field(4 + count_len + 8 * i, 8) as u8)
            .collect();
        (ecl, String::from_utf8(bytes).unwrap())
    }

    #[test]
    fn codes_read_back_at_every_level_and_across_versions() {
        let texts = [
            "tricky3D".to_string(),
            "https://example.com/parts?id=42&size=M6".to_string(),
            "Ünïcödé text: grüße, привет, 日本".to_string(),
            "x".repeat(120),
            "0123456789".repeat(60),
        ];
        for ecl in [Ecc::Low, Ecc::Medium, Ecc::Quartile, Ecc::High] {
            for text in &texts {
                let code = encode(text, ecl).unwrap();
                assert_eq!(code.size, code.version * 4 + 17);
                assert_eq!(
                    read(&code),
                    (ecl, text.clone()),
                    "{ecl:?} v{}",
                    code.version
                );
            }
        }
        // The smallest version that holds it.
        assert_eq!(encode("tricky3D", Ecc::Medium).unwrap().version, 1);
        assert_eq!(encode(&"a".repeat(2953), Ecc::Low).unwrap().version, 40);
        assert!(
            encode(&"a".repeat(2954), Ecc::Low)
                .unwrap_err()
                .starts_with("The text is too long")
        );
    }

    #[test]
    fn finder_and_timing_patterns_are_where_the_standard_puts_them() {
        let code = encode("tricky3D part library", Ecc::Quartile).unwrap();
        let n = code.size;
        for (x0, y0) in [(0, 0), (n - 7, 0), (0, n - 7)] {
            for d in 0..7 {
                // The finder's dark outer ring.
                assert!(code.dark(x0 + d, y0) && code.dark(x0, y0 + d));
                assert!(code.dark(x0 + d, y0 + 6) && code.dark(x0 + 6, y0 + d));
            }
            assert!(code.dark(x0 + 3, y0 + 3));
        }
        for i in 8..n - 8 {
            assert_eq!(code.dark(i, 6), i % 2 == 0);
            assert_eq!(code.dark(6, i), i % 2 == 0);
        }
        // The dark module beside the lower-left finder.
        assert!(code.dark(8, n - 8));
    }
}
