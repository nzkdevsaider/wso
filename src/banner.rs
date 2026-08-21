pub const BANNER: &str = "\
██     ██ ▄█████ ▄████▄\n\
██ ▄█▄ ██ ▀▀▀▄▄▄ ██  ██\n\
\u{a0}▀██▀██▀  █████▀ ▀████▀";

pub const VERSION: &str = env!("WSO_VERSION");

/// The art with the version below it. The version is right-aligned to the width
/// of the art. The indent characters are `U+00A0` because clap removes plain
/// leading spaces from each line of `before_help`.
pub fn header() -> String {
    format!("{BANNER}\n{}", version_line(VERSION))
}

fn version_line(version: &str) -> String {
    let indent = art_width().saturating_sub(version.chars().count());
    format!("{}{version}", "\u{a0}".repeat(indent))
}

fn art_width() -> usize {
    BANNER
        .lines()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or_default()
}

pub fn should_print(stdout_is_terminal: bool, yes: bool) -> bool {
    stdout_is_terminal && !yes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_attended_terminal_gets_the_banner() {
        assert!(should_print(true, false));
        assert!(!should_print(true, true));
        assert!(!should_print(false, false));
        assert!(!should_print(false, true));
    }

    #[test]
    fn clap_trims_a_plain_leading_space_so_the_indent_is_a_non_breaking_one() {
        let indented = BANNER.lines().nth(2).expect("the banner has three lines");
        assert!(indented.starts_with('\u{a0}'));
        assert!(!BANNER.lines().any(|line| line.starts_with(' ')));
    }

    #[test]
    fn every_line_is_the_same_width() {
        let widths: Vec<usize> = BANNER.lines().map(|line| line.chars().count()).collect();
        assert_eq!(widths, vec![23, 23, 23]);
    }

    #[test]
    fn the_banner_is_three_lines_with_no_trailing_newline() {
        assert_eq!(BANNER.lines().count(), 3);
        assert!(!BANNER.ends_with('\n'));
    }

    #[test]
    fn the_header_puts_the_version_under_the_art() {
        let rendered = header();
        let lines: Vec<&str> = rendered.lines().collect();
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[..3], BANNER.lines().collect::<Vec<_>>()[..]);
        assert_eq!(lines[3].trim_start_matches('\u{a0}'), VERSION);
    }

    #[test]
    fn the_version_line_is_right_aligned_with_non_breaking_spaces() {
        assert_eq!(
            version_line("0.1.0"),
            format!("{}0.1.0", "\u{a0}".repeat(18))
        );
        assert!(!header().lines().any(|line| line.starts_with(' ')));
    }

    #[test]
    fn a_version_wider_than_the_art_is_not_indented() {
        let long = "1.0.0-dev.0123456789abcdef";
        assert_eq!(version_line(long), long);
    }
}
