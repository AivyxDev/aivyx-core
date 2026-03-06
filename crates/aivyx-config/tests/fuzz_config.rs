use proptest::prelude::*;

proptest! {
    #[test]
    fn fuzz_config_toml(s in "\\PC*") {
        let _ = toml::from_str::<aivyx_config::AivyxConfig>(&s);
    }
}
