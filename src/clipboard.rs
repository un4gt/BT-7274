//! Terminal clipboard writes through OSC 52, including tmux passthrough.

use std::io::{self, Write as _};

use base64::{Engine as _, engine::general_purpose::STANDARD};

const MAX_CLIPBOARD_BYTES: usize = 1_048_576;

pub fn copy(text: &str) -> io::Result<()> {
    if text.len() > MAX_CLIPBOARD_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("clipboard payload exceeds {MAX_CLIPBOARD_BYTES} bytes"),
        ));
    }
    let sequence = osc52_sequence(text, std::env::var_os("TMUX").is_some());
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    stdout.write_all(sequence.as_bytes())?;
    stdout.flush()
}

fn osc52_sequence(text: &str, tmux: bool) -> String {
    let encoded = STANDARD.encode(text.as_bytes());
    let osc = format!("\x1b]52;c;{encoded}\x07");
    if tmux {
        format!("\x1bPtmux;{}\x1b\\", osc.replace('\x1b', "\x1b\x1b"))
    } else {
        osc
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc52_is_base64_encoded_and_tmux_escaped() {
        let direct = osc52_sequence("你好", false);
        assert!(direct.starts_with("\x1b]52;c;"));
        assert!(direct.ends_with('\x07'));
        assert!(!direct.contains("你好"));

        let tmux = osc52_sequence("copy", true);
        assert!(tmux.starts_with("\x1bPtmux;"));
        assert!(tmux.ends_with("\x1b\\"));
        assert!(tmux.contains("\x1b\x1b]52;c;"));
    }
}
