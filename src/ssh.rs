use ssh2::Session;
use std::io::Read;
use std::net::TcpStream;
use std::path::Path;
use std::time::Duration;

/// Fetches the content of a file from a remote server using SSH.
///
/// This function connects to a remote server, authenticates using either a private key
/// or an SSH agent, and then executes a `cat` command to retrieve the content of the
/// specified file. It includes timeouts for the connection and SSH operations.
///
/// # Arguments
///
/// * `server_name` - The name of the server, used for logging.
/// * `server_address` - The address of the server to connect to.
/// * `user` - The username to authenticate with.
/// * `remote_path` - The absolute path of the file to fetch on the remote server.
/// * `identity_file` - An optional path to an SSH private key file for authentication.
///
/// # Returns
///
/// A `Result` containing the file content as a `Vec<u8>` if successful,
/// or an `anyhow::Error` if the connection, authentication, or file retrieval fails.
pub fn fetch_remote_file(
    server_name: &str,
    server_address: &str,
    user: &str,
    remote_path: &str,
    identity_file: Option<&str>,
    password: Option<&str>,
) -> Result<Vec<u8>, anyhow::Error> {
    log::info!("[{}] Attempting to connect to {}", server_name, server_address);

    let addr = format!("{}:22", server_address);
    let tcp = TcpStream::connect_timeout(&addr.parse()?, Duration::from_secs(10))?;
    let mut session = Session::new()?;
    session.set_tcp_stream(tcp);
    session.set_timeout(30000); // 30 seconds for SSH operations
    session.handshake()?;
    log::debug!("[{}] Handshake complete", server_name);

    if let Some(key_path) = identity_file {
        log::info!("[{}] Authenticating with private key: {}", server_name, key_path);
        session.userauth_pubkey_file(user, None, Path::new(key_path), None)?;
    } else if let Some(pw) = password {
        log::info!("[{}] Authenticating with password", server_name);
        session.userauth_password(user, pw)?;
    } else {
        log::info!("[{}] Authenticating with SSH agent", server_name);
        session.userauth_agent(user).map_err(|e| {
            anyhow::anyhow!(
                "No password or identity file configured for '{}'. \
                 SSH agent authentication failed: {}. \
                 Use 'c' in the dashboard to add credentials.",
                server_name, e
            )
        })?;
    }
    log::info!("[{}] Authentication successful", server_name);

    let (command, use_sudo) = if password.is_some() {
        (format!("sudo -S cat {}", remote_path), true)
    } else {
        (format!("cat {}", remote_path), false)
    };

    let mut channel = session.channel_session()?;
    channel.exec(&command)?;

    if use_sudo {
        use std::io::Write;
        channel.write_all(format!("{}\n", password.unwrap()).as_bytes())?;
    }

    let mut contents = Vec::new();
    channel.read_to_end(&mut contents)?;
    log::debug!(
        "[{}] Successfully read {} bytes from stdout.",
        server_name,
        contents.len()
    );

    let mut stderr = String::new();
    channel.stderr().read_to_string(&mut stderr)?;
    channel.wait_close()?;
    let exit_code = channel.exit_status()?;

    if exit_code != 0 {
        anyhow::bail!(
            "[{}] Remote command failed with exit code {}. Stderr: {}",
            server_name,
            exit_code,
            stderr.trim()
        )
    }

    Ok(contents)
}
