use std::{
    env, fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{exit, Command},
};

use which::which;

/// Main entry point for the DBC extractor CLI tool.
///
/// This function checks for required DBC files, and if missing, attempts extraction,
/// and will exit with an error if extraction fails or required files are still missing.
fn main() {
    let env = env_logger::Env::default().filter_or("RUST_LOG", "warn");
    env_logger::init_from_env(env);

    let dbc_folder_path = get_dbc_folder_path();

    if ensure_dbc(&dbc_folder_path) {
        log::info!("DBC files found");
        return;
    }

    extract_dbc(&dbc_folder_path);

    if !ensure_dbc(dbc_folder_path) {
        log::error!("Required DBC files still missing");
        exit(1);
    }
}

/// Returns the path to the DBC folder.
///
/// Uses the `DBC_FOLDER_PATH` environment variable if set, otherwise defaults to `<workspace>/dbc`.
fn get_dbc_folder_path() -> PathBuf {
    match env::var("DBC_FOLDER_PATH") {
        Ok(env_str) => PathBuf::from(env_str),
        Err(_) => {
            let workspace_path = get_workspace_path();
            let dbc_path = workspace_path.join("dbc");
            log::info!(
                "DBC_FOLDER_PATH not set, using default: {}",
                dbc_path.display()
            );
            dbc_path
        }
    }
}

/// Checks if the DBC directory exists and contains all required DBC files.
///
/// Returns `true` if all required files are present, otherwise `false`.
fn ensure_dbc<P: AsRef<Path>>(dbc_dir_path: P) -> bool {
    if !dbc_dir_path.as_ref().exists() {
        log::warn!(
            "DBC directory `{}` does not exist",
            dbc_dir_path.as_ref().display()
        );
        return false;
    }

    static REQUIRED_DBC_FILES: &[&str] = &[
        "ChrRaces.dbc",
        "ChrClasses.dbc",
        "Map.dbc",
        "CharStartOutfit.dbc",
        "AreaTrigger.dbc",
    ];

    for dbc_file in REQUIRED_DBC_FILES {
        let dbc_path = dbc_dir_path.as_ref().join(dbc_file);
        if !dbc_path.exists() {
            log::warn!("Required DBC file `{}` is missing", dbc_path.display());
            return false;
        }
    }

    true
}

/// Attempts to extract DBC files from WoW MPQ archives using `warcraft-rs`.
///
/// If `warcraft-rs` is not installed, prompts the user to install it.
/// Creates the DBC output directory if it does not exist. Iterates over all
/// required MPQ files and runs the extraction command for each, exiting on failure.
///
/// # Arguments
/// * `dbc_folder_path` - Path to the output DBC directory.
fn extract_dbc<P: AsRef<Path>>(dbc_folder_path: P) {
    let mut warcraft_rs = which("warcraft-rs");
    if warcraft_rs.is_err() {
        log::warn!("warcraft-rs not found in PATH. Would you like to install it?");
        if ask_yes_no("Would you run `cargo install warcraft-rs`?") {
            Command::new("cargo")
                .arg("install")
                .arg("warcraft-rs")
                .status()
                .expect("Failed to execute `cargo install warcraft-rs`");
        }
        warcraft_rs = which("warcraft-rs");
    }

    let warcraft_rs_path = warcraft_rs.expect("Failed to find `warcraft-rs` in PATH");
    log::debug!("warcraft-rs found at {}", warcraft_rs_path.display());

    let dbc_folder_path = dbc_folder_path.as_ref();
    if !dbc_folder_path.exists() {
        log::info!("Creating DBC folder at {}", dbc_folder_path.display());
        fs::create_dir_all(dbc_folder_path).expect("Failed to create DBC folder");
    }

    let extract_dbc_args = [
        "mpq",
        "extract",
        "-f",
        "dbc",
        "--output",
        dbc_folder_path.to_str().unwrap(),
    ];
    let wotlk_data_path = get_wotlk_path().join("Data");
    let wotlk_mpqs = get_mpq_paths(&wotlk_data_path);

    for wotlk_mpq in wotlk_mpqs {
        let wotlk_mpq_path = wotlk_data_path.join(&wotlk_mpq);
        log::info!("Extracting DBC from {}", wotlk_mpq_path.display());

        let status = Command::new("warcraft-rs")
            .args(extract_dbc_args)
            .arg(&wotlk_mpq)
            .current_dir(&wotlk_data_path)
            .status()
            .expect("Failed to execute `warcraft-rs`");
        if !status.success() {
            log::error!("warcraft-rs failed with exit code: {}", status);
            exit(1);
        }
    }
}

/// Returns the workspace root path.
///
/// Uses the `CARGO_MANIFEST_DIR` environment variable if set, otherwise defaults to current directory.
fn get_workspace_path() -> PathBuf {
    match env::var("CARGO_MANIFEST_DIR") {
        Ok(env_str) => PathBuf::from(env_str)
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf(),
        Err(_) => {
            log::info!("CARGO_MANIFEST_DIR not set, using cwd: ./");
            ".".into()
        }
    }
}

/// Returns the path to the WOTLK folder.
///
/// Uses the `WOTLK_PATH` environment variable if set, otherwise defaults to current directory.
fn get_wotlk_path() -> PathBuf {
    match env::var("WOTLK_PATH") {
        Ok(env_str) => PathBuf::from(env_str),
        Err(_) => {
            log::info!("WOTLK_PATH not set, using default: ./");
            ".".into()
        }
    }
}

/// Prompts the user for a yes/no answer in the terminal.
///
/// Returns `true` for 'y' or 'yes', otherwise `false`.
fn ask_yes_no(prompt: &str) -> bool {
    print!("{} [y/n]: ", prompt);
    io::stdout().flush().unwrap();

    let mut input = String::new();
    io::stdin().read_line(&mut input).unwrap();
    matches!(input.trim().to_lowercase().as_str(), "y" | "yes")
}

/// Returns a list of MPQ file names for the given WOTLK data path.
///
/// Checks for the existence of each MPQ file and exits if any are missing.
fn get_mpq_paths<P: AsRef<Path>>(wotlk_data_path: P) -> Vec<String> {
    let wotlk_data_path = wotlk_data_path.as_ref();
    if !wotlk_data_path.exists() {
        log::error!(
            "WOTLK data path `{}` does not exist",
            wotlk_data_path.display()
        );
        exit(1);
    }

    let wotlk_mpqs = [
        "{}/locale-{}.MPQ",
        "{}/patch-{}.MPQ",
        "{}/patch-{}-2.MPQ",
        "{}/patch-{}-3.MPQ",
    ];

    let locale = get_locale(wotlk_data_path);

    wotlk_mpqs
        .iter()
        .map(|mpq| {
            let mpq_file_name = mpq.replace("{}", locale);
            let mpq_path = wotlk_data_path.join(&mpq_file_name);
            if !mpq_path.exists() {
                log::error!("MPQ file `{}` not found", mpq_path.display());
                exit(1);
            }
            mpq_file_name
        })
        .collect()
}

/// Detects and returns the locale string for the given WOTLK data path.
///
/// Searches for known locale folders and returns the first found.
/// Exits if no locale is found.
fn get_locale<P: AsRef<Path>>(wotlk_data_path: P) -> &'static str {
    static LOCALES: &[&str] = &[
        "enUS", "deDE", "frFR", "esES", "itIT", "koKR", "zhCN", "zhTW", "ruRU",
    ];
    for locale in LOCALES {
        let locale_path = wotlk_data_path.as_ref().join(locale);
        if locale_path.exists() {
            return locale;
        }
    }
    exit(1);
}
