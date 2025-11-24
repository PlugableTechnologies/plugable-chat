use std::process::Command;
use std::env;
use std::path::Path;

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let project_root = Path::new(&manifest_dir).parent().unwrap();

    println!("cargo:rerun-if-changed=../src");
    println!("cargo:rerun-if-changed=../index.html");
    println!("cargo:rerun-if-changed=../package.json");
    println!("cargo:rerun-if-changed=../vite.config.ts");
    println!("cargo:rerun-if-changed=../tailwind.config.js");
    println!("cargo:rerun-if-changed=../postcss.config.js");

    let is_windows = cfg!(windows);
    let npm_cmd = if is_windows { "npm.cmd" } else { "npm" };

    // Ensure dependencies are installed
    if !project_root.join("node_modules").exists() {
        println!("cargo:warning=Installing frontend dependencies...");
        let status = Command::new(npm_cmd)
            .arg("install")
            .current_dir(project_root)
            .status()
            .expect("Failed to execute npm install");
        
        if !status.success() {
            panic!("Frontend dependency installation failed");
        }
    }

    // Generate icons before the frontend build so the Tauri bundler always has the assets it expects.
    // println!("cargo:warning=Generating icons...");
    // let status = Command::new(npm_cmd)
    //     .args(["run", "generate-icons"])
    //     .current_dir(project_root)
    //     .status()
    //     .expect("Failed to execute npm run generate-icons");

    // if !status.success() {
    //     panic!("Icon generation failed");
    // }

    // Run frontend build
    // We check for a specific env var to skip this if needed, but for now we run it to be "overarching"
    // println!("cargo:warning=Building frontend...");
    // let status = Command::new(npm_cmd)
    //     .args(["run", "build"])
    //     .current_dir(project_root)
    //     .status()
    //     .expect("Failed to execute npm run build");

    // if !status.success() {
    //     panic!("Frontend build failed");
    // }

    tauri_build::build()
}
