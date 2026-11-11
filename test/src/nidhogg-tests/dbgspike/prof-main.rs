// The spike's Seatbelt profile: prof SPEC.json DEVELOPER_DIR OUT.sb, through gleipnir's own validation and profile.
use std::path::PathBuf;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let spec: gleipnir::Spec = serde_json::from_str(&std::fs::read_to_string(&a[1]).unwrap()).unwrap();
    let home = std::fs::canonicalize(std::env::var("HOME").unwrap()).unwrap();
    let ctx = gleipnir::Context {
        home,
        closed: vec![],
        open_inside_closed: vec![],
        user_sockets: vec![],
        run_parents: vec![],
        program_dirs: vec![],
        env_deny: vec![],
    };
    let v = match gleipnir::validate(spec, &ctx, gleipnir::Target::Macos) {
        Ok(v) => v,
        Err(r) => {
            eprintln!("refused: {r:?}");
            std::process::exit(3);
        }
    };
    let mac = gleipnir::sbpl::MacContext {
        developer_dir: Some(PathBuf::from(&a[2])),
        users_dir: PathBuf::from("/Users"),
    };
    std::fs::write(&a[3], gleipnir::sbpl::profile(&v, &mac).unwrap()).unwrap();
}
