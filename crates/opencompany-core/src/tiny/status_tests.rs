use super::*;

#[test]
fn all_known_modules_are_reported() {
    let modules = RuntimeModuleStatus::all();
    let names: Vec<_> = modules.iter().map(|module| module.name).collect();

    assert_eq!(names, vec!["tinyagents", "openhuman"]);
    assert_eq!(modules[0].path, "vendor/openhuman/vendor/tinyagents");
}
