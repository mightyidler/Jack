use std::path::PathBuf;

pub fn core_props_candidates() -> Vec<PathBuf> {
    std::env::var_os("PROGRAMDATA")
        .map(PathBuf::from)
        .map(|root| {
            vec![
                root.join("SteelSeries/GG/coreProps.json"),
                root.join("SteelSeries/SteelSeries Engine 3/coreProps.json"),
            ]
        })
        .unwrap_or_default()
}
