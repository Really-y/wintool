//! Banner art (generated from a 24x26 PNG, Windows console 16-color palette).
//! One string per pixel row; each char is a palette index 0-f, '.' = transparent.
//! Two pixel rows are drawn per text row with the half-block characters.

pub const WIDTH: usize = 24;

pub const PIXELS: &[&str] = &[
    "...8.88887777888880.....",
    ".8ffffffff777777778.....",
    "0fffff7788777777778.....",
    "0888887fffffffffff7.....",
    "8fff78ffffffffff7e7.....",
    "8fff78ffff7aa772222.....",
    "8fff77fff822222222280...",
    "8fff77ff82aa808f78077...",
    "0fff77f72aaa808778088...",
    "0fff77f2aaaa222226660...",
    "0fff7772aaaa26666c4c4...",
    "0fff7772aaaa666666660...",
    "0fff87b2aaaaaaaaaaa31...",
    "0fff839933222333999111..",
    "0fff7311111111111111190.",
    "0fff7311191111111111132.",
    "07ff70331228139333333220",
    "878880aaa2222222aaaaaa20",
    "0fff7822aaa2222222202aa0",
    ".8fff782222222aa20880aa0",
    "..7fff77788880aa28772a2.",
    "..7ffffffff7f82a27f22a0.",
    "...ffffffffff8a28f72a0..",
    "...ffffff88882a27f02a2..",
    "...ffffffff788228f802a0.",
    "...ffffffff77f808f0.000.",
];
