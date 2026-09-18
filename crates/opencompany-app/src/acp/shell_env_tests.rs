use super::*;

/// The case this module exists for: a version manager greeting the user on
/// every shell start, interleaved with the answer.
#[test]
fn a_chatty_rc_file_does_not_corrupt_the_answer() {
    let noisy = format!("Now using node v24.0.0\n{DELIM}/opt/homebrew/bin:/usr/bin{DELIM}\n");
    assert_eq!(
        parse_path(&noisy).as_deref(),
        Some("/opt/homebrew/bin:/usr/bin")
    );
}

#[test]
fn output_with_no_delimiters_is_refused() {
    // A shell that failed before reaching the `printf` still exits 0 and
    // still prints its complaint. Reading that as a `PATH` would produce a
    // lookup against nonsense directories.
    assert_eq!(parse_path("zsh: command not found: nvm\n"), None);
}

#[test]
fn a_half_written_answer_is_refused() {
    // Killed mid-write on the timeout path: the opening delimiter arrived
    // and the closing one never did, so the tail is a truncated `PATH`.
    assert_eq!(parse_path(&format!("{DELIM}/opt/homebrew/b")), None);
}

#[test]
fn an_empty_path_is_refused_rather_than_used() {
    // `$PATH` unset inside the rc file. An empty answer would resolve every
    // harness to "not installed" — the exact failure being fixed here.
    assert_eq!(parse_path(&format!("{DELIM}{DELIM}")), None);
}

#[test]
fn an_answer_that_names_no_directory_is_refused() {
    // Guards against a shell echoing the literal word rather than expanding
    // it, which is otherwise a plausible-looking non-empty string.
    assert_eq!(parse_path(&format!("{DELIM}$PATH{DELIM}")), None);
}

/// There is always an answer, so a harness is never reported missing merely
/// because the shell could not be consulted.
#[test]
fn there_is_always_a_path_to_fall_back_on() {
    assert!(effective_path().is_some_and(|p| !p.is_empty()));
}
