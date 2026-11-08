//! Banner art: a 16-color version (Windows 7 legacy console) and a truecolor version.
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

/// Truecolor version for consoles with VT support (Windows 10+): 6 hex digits (RRGGBB)
/// per pixel, "......" = transparent.
pub const RGB_WIDTH: usize = 32;

pub const RGB_PIXELS: &[&str] = &[
    "................................................3236383a3e415254556466667d7d7d8688888183838183837d7d7d686b6a585a5b4f5153424446303436262a2b15191c0f1217..........................................",
    "............707474b3b3b3d3d3d3d3d3d3d4d6d6cbcdcdd9d9d9ebebebfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffafafae9e9e9e4e3e4cccccd55595b..........................................",
    "......747677ffffffffffffffffffffffffffffffffffffffffffffffffe9ececbcbcbc9497978686867e80817679795659595456566d6d6d6c6c6c5c5e5e6969697b7b7bb2b1af8c8e8f..........................................",
    "25292cdde0dffffffff6f7f3faf9faf6f6f4fffffddedfdc9d9f9d7e7e7e7476768e8e8ea3a6a6b5b5b5c7c9c7d1d1d1d0d0d0d7d7d7e3e3e1e3e3e1d8d9d5cecfcbafb1afa0a19e5a5e5e..........................................",
    "1a1d1e737672767b756d6f6f6464656668684d50505d5f5fafafafd7d7d7ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffdddadd..........................................",
    "484b4ce7e9e7eff0edeff1eff0f0f0ffffff6b6e6e939593fffffffffffffffffffdfcfcfafaf7fafaf8fffffffffffffffffffffffffcfcfafefffcfffffffffffff3faf2ffffffdee3dc..........................................",
    "474a4bffffffffffffffffffffffffffffff727472989b95fffffffafafafefefefdfdf7fffefbfffffff3fbead0ddc6cadcc1e7f8dcfafff1f4ffecbed0b58da77d7f9e6c96b8858da880..........................................",
    "3d4041f1f1f1fffffffbfbfbfbfbfbffffff6d706daaaeaafffffffbfbfbfefefffffffffbfff4b2c8a471965f557f404671305e884b82a36e779864587f43527b354d7e32487b2f446c2e2e4621....................................",
    "3f4243f2f2f2fffffffafdfdfefefeffffff757973bec1bdfffffffcfbfcfffffff2fced8fae83507e3c497d2b396d1944772b648e566d8e596584534c6e3b4a70314a742d507939688a56849e774e6248..............................",
    "313335e7e7e7fffffffcfcfcfefefeffffff72766fc2c5c1fffffffdfdfbfcfff792b0834981325b973d55923260904595b3885d6f56525f4cabb7a6d9e2d17b916d889f77899c803b50369eb19bf7ffeea0a49f........................",
    "24272bd2d4d4fffffffcfcfcfdfffdffffff5e6060c9c9c7fffffffffff9abc49d4c7e33589b375c9f3c50902f8fb67ae0edd4182014000000858e80ffffffbed2b3f8ffed83927f000000495c46ffffffdae4d6........................",
    "23282ac9cbcbfffffffcfcfcffffffffffff57595ad0d0d0ffffffb8ccae416c2c518a2d62a23a5b9d395796385a8e4184a6734a623e324c2a94b28a94b3886086508db07d7998712a44256684558ea870536245........................",
    "24292bc5c8c5fffffffdfcfcfefefeffffff585858d3d7d1fffffc66875e4f84315d9a345f9d39599d36589c395b993d64983e6e9641739a4b648a396283336e8136757d34807c3d898245847735756b33383a1d........................",
    "22262ac3c3c3fffffffbfbfbf9fcfcffffff565954d6dbd3cfe5c24b7d365f9d3d5ea038599d3a589d395a9d3e61963c65822f6b74287c692b886e2d8c712d916a289a68299761269858259d582a9e683b533c25........................",
    "212528babcbcfffffffbfbfbfefefeffffff4c4e4fe1e4e5acc5a1437a28609e395b9d34589e32589f32599f336999396f7b2b7b6a258e6627926026955e259d63279a6829986e2f9870309b71328d6d3e4c3c26........................",
    "212428bbbdbdfffffffbfbfbfffffffffefe404245e8e6e9adc2a4497d2e65a03a609d345f9d395a9f35589d305b912d6f8a347b8033817d35847c36887b34867c347f843a75883e69943c77a7545d7f53213a34........................",
    "222528c5c5c6fffffffbfbfbfffffff9fdf5393e38e8e4e396afac417e5d66a76662a04a629d415f9e3f5c9e3b5a9b375e9a3b689d42729b3a749d3d739b436e9e4d659d59619f5f58997959979924557201245c09235b..................",
    "0e1014acaeacfffffffbfbfbfffffff8fcf5414440d3daeb486fb31957a8498eb5559a8f5b9e755ea1745fa06e63a16f62a275599b6f5c9b76559779519488498f9e428bb43780b51f65c71554d70237bd164ed4073396052158............",
    "0c0e139c9e9ffffffffafafafffffffdffff56565495a5c8033cbd0148ff064ae1134ca82663aa2e6fba2968b02365ab2064ab1c5dad1459d41056d90c54e4044ff0014dff024cff004cff004eff003af80743f72057ca0c2853............",
    "181d1fb1b3b4fffffff9f9f9ffffffffffff5c5c57687fa70042d60052ff0253ff003ad20025a7002ab30a46dc064eff004dff004dff004bff004bff004cff004cff004cff004fff0251ff0051ff004cff0032de5a8cc652695d............",
    "222628dbdbdcfffffffdfdfdfdfdfdfcfcfd686565657eab053cae0545ea0035dd0941c5417bbf3d759c0b3f7e043eca0951ff044fff024bff0049fe0046fa0047f90045f80045f70045f90045ff024cff0037b13e746e7aa1670c1f05......",
    "181b1e6e6e6fd6d6d4ffffffffffffffffff4d4c4c1525274379834280912d6281113d522d6145639d636aa774366c6c164d6f33749d3c7cb23577b04189bb3f85b73b82b13a82b03a81ab377fa63b82a93672841e50224b76366882612b3729",
    "464849b0b0b17272727375739a9a9adedfdb36383a000e0084b1736faa506dab4f6ea6514c7a2c24510d335d1f5c8f4c6498593e70352b5d202d5c1e41713574b06d6dab686ba9646baa646dad546daf4f69ab636fa85e3a6f1f4b723c31442d",
    "26292ae7e9e9ffffffd8d8d89496966565680c0e1200000070965c7cac596fa3446fa84477b04d74a85058863e396426305a1e51813c6b9d4b75a75417440327541274a456709e5478a3616899485a91336ca2516ea64a71ac4457893d182d13",
    "......6d7171ffffffffffffffffffffffffccd0cb6e81672a461d3d602e4d7838609148629943639d3469a0416e9d5f58854a42752b33622031611e4775374167321133071c3d12142b0c0419000000002948206ba04762a03c77a66121361a",
    "............b5b7b7fffffff8f8f6ffffffa8abab999d9996a09165755e576b4f465c3d39533235492e2d3f2629392620361b44653675a1596da64a72ad487aa85a142a0c4c524a6c706988888574767614340a5b9c3e6baf4e5c8548111d0f",
    "............595c5cfffffffffffffdfdfee8e7e8b2b4b29aa1998f9d8c8895828998848c9887adb2acb7bbb6b3b8b29fab9b142710325a1e77af536ca541648d4643573be2e5e0e4e8e3f4f4f2abb3a73f662a6dab5079b5622b4b20......",
    "..................c4c4c4fffffff7f7f5fffffffffffffffffff3f3eee2e7dfd6d9d2b7b8b59395938e8d8e878788a0a49f7a81790d3301669d4678ae533d622b3945369fa29d9c9f9e7c837b0c22035585397bb65f608651041503......",
    "..................a5a5a5fffffffffffcfbfbf8fafbf7fefdfbffffffffffffffffffffffffffffffffffffffffffffffffe7eee5284e18669d4972aa4f234318c1c5c1ffffffffffff98a6953357227caf596fa05413290c............",
    "..................949296fffffffffffffefaf9fffdfbfdfdfafbfbfcfffeffe3e5e3ccd1cafdfef7ffffffffffffffffffb9c4b63051217aae5a598f3939532ffdfbfffffefebabfb62e432668944e73ab5019410d..................",
    "........................eef1eefffffffefaf9fffdfbfefdfbfbfbf8ffffffd8d8d66c6c6c6769678586839da19cc3c6c35e666142633583b85d387119597053ffffffa5a9a3434d3e26431b81b76469a24b204416041002............",
    "........................f4f7f3fffffffcfbf9fffdf9fcfbf9fafdf8fcfcf9fffffffefefad9d9d5c7c7c5737673696c68757b772a4b1d7eaa635c8a442e4526edefedffffff818980000900638e4d82bc6682b36b243c1d............",
    "........................fdfdfbffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffa7aca79c9f9affffff657c5c3f5a307998692d4125b1b5b0ffffff57595a......19311341673087b477395033............",
    "........................e8e8e8f3f5f6eef0f1f1f4f4f5f7f7f6f6f8f6f6f8f4f6f7f4f4f6eff1f4f8f8f88d908f898b89f4f6f6e7f0e699a795374a31465045d2d4d4e4e7e6444647............0a1a081f341e162017............",
];
