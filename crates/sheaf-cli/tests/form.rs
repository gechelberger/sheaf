//! `sheaf-cli form check` end to end, through the binary.

use std::path::PathBuf;
use std::process::Command;

use lopdf::Object;
use sheaf_doc::forms::fixture::Form;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sheaf-cli-form-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn sheaf(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_sheaf-cli"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn check_prints_the_form_and_its_problems() {
    let dir = scratch("check");
    let mut form = Form::new(1);
    let name = form.text("name", 0, [72.0, 680.0, 272.0, 700.0]);
    form.fill(name, "Ann Lee");
    form.check_box("agree", 0, [72.0, 650.0, 92.0, 670.0], true);
    form.usage_rights();
    let clean = dir.join("clean.pdf");
    std::fs::write(&clean, form.bytes()).unwrap();

    let arial = form.text("arial", 0, [72.0, 620.0, 272.0, 640.0]);
    form.dict(arial)
        .set("DA", Object::string_literal("/ArialMT 9 Tf 0 g"));
    // The check box leaves /Fields.
    form.set_fields(&[name, arial]);
    let broken = dir.join("broken.pdf");
    std::fs::write(&broken, form.bytes()).unwrap();

    let run = sheaf(&["form", "check", clean.to_str().unwrap()]);
    assert!(run.status.success());
    let out = String::from_utf8(run.stdout).unwrap();
    assert_eq!(
        out,
        "AcroForm: 2 fields, 2 widgets, 1 page\n\
         Reader usage rights (/Perms /UR3)\n\
         SigFlags 2\n\
         No problems found.\n"
    );

    let run = sheaf(&["form", "check", broken.to_str().unwrap()]);
    assert!(run.status.success());
    let out = String::from_utf8(run.stdout).unwrap();
    assert!(
        out.ends_with(
            "Problems:\n\
             - 1 widget no field in /Fields reaches (e.g. \"agree\")\n\
             - fonts /DA names that /DR lacks: ArialMT ×1\n"
        ),
        "{out}"
    );

    let run = sheaf(&["form", "check", dir.join("none.pdf").to_str().unwrap()]);
    assert_eq!(run.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&run.stderr).starts_with("error: reading"));
    assert!(sheaf_cli::is_subcommand("form".as_ref()));
    let _ = std::fs::remove_dir_all(&dir);
}
