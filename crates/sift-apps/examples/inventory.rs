//! Print a compact environment summary for manual inspection.
//!
//! cargo run -p sift-apps --example inventory -- /Applications ~

fn main() {
    let roots: Vec<std::path::PathBuf> = std::env::args()
        .skip(1)
        .map(std::path::PathBuf::from)
        .collect();
    let context = sift_apps::detect(&roots, dirs::home_dir().as_deref());
    println!("apps: {}", context.apps.len());
    for app in context.apps.iter().take(10) {
        println!(
            "  {} {} {}",
            app.name,
            app.version.clone().unwrap_or_default(),
            app.install_path
        );
    }
    println!("package managers: {}", context.package_managers.len());
    for pm in &context.package_managers {
        println!(
            "  {} {} projects={} caps={:?}",
            pm.id,
            pm.version.clone().unwrap_or_default(),
            pm.projects.len(),
            pm.capabilities
        );
    }
    println!("projects: {}", context.projects.len());
    for project in context.projects.iter().take(10) {
        println!(
            "  {} {} {:?}",
            project.root, project.pm, project.lockfile
        );
    }
    println!("hints: {}", sift_apps::hints_for(&context).len());
}
