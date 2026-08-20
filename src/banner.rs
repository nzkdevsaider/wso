pub const BANNER: &str = "\
██     ██ ▄█████ ▄████▄\n\
██ ▄█▄ ██ ▀▀▀▄▄▄ ██  ██\n\
\u{a0}▀██▀██▀  █████▀ ▀████▀";

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
}
