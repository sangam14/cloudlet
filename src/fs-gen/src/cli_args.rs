use std::{env, path::PathBuf};

use clap::{error::ErrorKind, ArgAction, CommandFactory, Parser};
use clap_stdin::MaybeStdin;
use regex::Regex;

use once_cell::sync::Lazy;

// So, for any of you who may be scared, this is the regex from the OCI Distribution Sepcification for the image name + the tag
static RE_IMAGE_NAME: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*(/[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*)*(?::[a-zA-Z0-9_][a-zA-Z0-9._-]{0,127})?").unwrap()
});

/// Convert an OCI image into a CPIO file
#[derive(Parser, Debug, Clone)]
#[command(version, about, long_about = None)]
pub struct CliArgs {
    /// The name of the image to download, can include repository and tag: [REPOSITORY/NAME:TAG]
    pub image_name: String,

    /// The host path to the guest agent binary
    pub agent_host_path: PathBuf,

    /// The path to the output file
    #[arg(short='o', long="output", default_value=get_default_output_file().into_os_string())]
    pub output_file: PathBuf,

    /// The path to the temporary folder
    #[arg(short='t', long="tempdir", default_value=get_default_temp_directory().into_os_string())]
    pub temp_directory: PathBuf,

    #[arg(short='i', long="init", default_value=None)]
    pub initfile_path: Option<PathBuf>,

    #[arg(long = "arch", default_value = "amd64")]
    pub architecture: String,

    #[arg(short='d', long="debug", action=ArgAction::SetTrue)]
    pub debug: bool,

    /// Username to pull image from a private repository
    #[arg(short='u', long="username", default_value=None)]
    pub username: Option<String>,

    /// Password can also be passed via STDIN: [echo <PASSWORD> | fs-gen ... -p -]
    #[arg(short='p', long="password", default_value=None)]
    pub password: Option<MaybeStdin<String>>,

    /// Allow invalid TLS certificates
    #[arg(long="insecure", action=ArgAction::SetTrue)]
    pub insecure: bool,

    /// Disable the compression of the final image
    #[arg(short, long, action=ArgAction::SetTrue)]
    pub no_compression: bool,
}

impl CliArgs {
    /// Get the cli arguments with additional validation
    pub fn get_args() -> Self {
        let args = CliArgs::parse();

        args.validate_image();
        args.validate_host_path();
        args.validate_auth();

        args
    }

    fn validate_image(&self) {
        if !RE_IMAGE_NAME.is_match(&self.image_name) {
            let mut cmd = CliArgs::command();
            cmd.error(
                ErrorKind::InvalidValue,
                format!("Invalid image name: \"{}\"", self.image_name),
            )
            .exit();
        }
    }

    fn validate_host_path(&self) {
        if !self.agent_host_path.exists() {
            let mut cmd = CliArgs::command();
            cmd.error(
                ErrorKind::InvalidValue,
                format!(
                    "File not found for agent binary: \"{}\"",
                    self.agent_host_path.to_string_lossy()
                ),
            )
            .exit();
        }
    }

    fn validate_auth(&self) {
        let mut cmd = CliArgs::command();
        let instruction =
            "Define both username and password to connect to a private image repository.";
        if self.username.is_none() && self.password.is_some() {
            cmd.error(
                ErrorKind::InvalidValue,
                format!("Username not provided. {}", instruction),
            )
            .exit();
        }
        if self.username.is_some() && self.password.is_none() {
            cmd.error(
                ErrorKind::InvalidValue,
                format!("Password not provided. {}", instruction),
            )
            .exit();
        }
    }
}

/// Get the default temporary directory for the current execution.
fn get_default_temp_directory() -> PathBuf {
    PathBuf::from("/tmp/cloudlet-fs-gen")
}

/// Get the default output file path for the generated initramfs.
fn get_default_output_file() -> PathBuf {
    let mut path = env::current_dir().unwrap();
    path.push("initramfs.img");
    path
}
