#![allow(dead_code)]

pub const PRODUCER_SOURCE_NAME: &str = "PairFactory.scala";
pub const PRODUCER_SOURCE_SHA256: &str =
    "c77b2b3e00bf602b86be30b29ceaa2300398feb34e80d2f76c37f391bb4d654f";
pub const JAVA_CLIENT_SHA256: &str =
    "97cfe2c30ff7cb0b07627ce83dd2392aaee898c4690bc70b40206200a37d4a1b";
pub const SCALA_SUPPORT_SOURCE_SHA256: &str =
    "06266874ed9a7acdfc4d6a1c27bea3967055e474bca5a2f666af5ebfc8669c28";

pub const TASTY_UUID_LINKS: [(&str, &str); 3] = [
    ("example/Counter.class", "008071c3c00968ac0000000000008193"),
    ("example/Pair.class", "000b7d8d7441c7f00000000000008198"),
    (
        "example/PairFactory.class",
        "00d07c03e8e3c6dc00000000000081a1",
    ),
];

pub const ARTIFACT_SHA256: [(&str, &str); 6] = [
    (
        "example/Counter.class",
        "fe3d5bc2169b6e02e28fec36271518d476e1b3aea7ac65077a8d382958f7ef6b",
    ),
    (
        "example/Counter.tasty",
        "3d6289dd35b38698347de93f679605547ac61ca1c87842b8dca96ca7a8293045",
    ),
    (
        "example/Pair.class",
        "77257daed358c452f0291b26bcc7d4a2778483b47df0b1861948b13e43b8e317",
    ),
    (
        "example/Pair.tasty",
        "4a7dcc31a6f771af5685afa6464bad85acdc869e7fec2feb9b3d41ee98dfd284",
    ),
    (
        "example/PairFactory.class",
        "351d311b48edb38d5b86b2621cb49568df508c944929633e4bdef35ea03d159c",
    ),
    (
        "example/PairFactory.tasty",
        "327e42e222d13f21fe8fbe7f7b8ddc769abf31a85c1d97d9b6115ed5d39efe5e",
    ),
];
