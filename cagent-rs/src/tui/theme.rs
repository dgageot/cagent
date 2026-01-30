//! Theme system for the TUI
//!
//! Provides multiple color schemes including dark, light, and high-contrast themes.
//! Supports automatic detection of terminal background color.

use ratatui::style::Color;

/// A complete color theme for the TUI
#[derive(Debug, Clone)]
pub struct Theme {
    pub name: &'static str,
    pub description: &'static str,

    // Primary colors
    pub primary: Color,
    pub accent: Color,
    pub warning: Color,
    pub error: Color,
    pub success: Color,

    // Role-specific colors
    pub user: Color,
    pub assistant: Color,
    pub tool: Color,
    pub brand: Color,

    // Text colors
    pub text_primary: Color,
    pub text_secondary: Color,

    // Background colors
    pub background: Color,
    pub background_alt: Color,
    pub border: Color,

    // Is this a dark theme?
    pub is_dark: bool,
}

impl Theme {
    /// Dark theme (default) - matches Go cagent
    pub fn dark() -> Self {
        Self {
            name: "dark",
            description: "Default dark theme",

            primary: Color::Rgb(122, 162, 247), // #7AA2F7 - Accent blue
            accent: Color::Rgb(158, 206, 106),  // #9ECE6A - Success green
            warning: Color::Rgb(224, 175, 104), // #E0AF68 - Warning yellow
            error: Color::Rgb(247, 118, 142),   // #F7768E - Error red
            success: Color::Rgb(158, 206, 106), // #9ECE6A - Success green

            user: Color::Rgb(125, 207, 255), // #7DCFFF - Info cyan
            assistant: Color::Rgb(158, 206, 106), // #9ECE6A - Success green
            tool: Color::Rgb(176, 131, 234), // #B083EA - Badge purple
            brand: Color::Rgb(29, 99, 237),  // #1D63ED - Brand blue

            text_primary: Color::Rgb(192, 192, 192), // #C0C0C0
            text_secondary: Color::Rgb(128, 128, 128), // #808080

            background: Color::Rgb(28, 28, 34),     // #1C1C22
            background_alt: Color::Rgb(38, 38, 48), // #262630
            border: Color::Rgb(107, 117, 168),      // #6B75A8

            is_dark: true,
        }
    }

    /// Light theme
    pub fn light() -> Self {
        Self {
            name: "light",
            description: "Light theme for bright environments",

            primary: Color::Rgb(56, 108, 176), // #386CB0 - Darker blue
            accent: Color::Rgb(80, 161, 79),   // #50A14F - Forest green
            warning: Color::Rgb(193, 132, 1),  // #C18401 - Amber
            error: Color::Rgb(228, 86, 73),    // #E45649 - Red
            success: Color::Rgb(80, 161, 79),  // #50A14F - Forest green

            user: Color::Rgb(1, 132, 188),      // #0184BC - Cyan
            assistant: Color::Rgb(80, 161, 79), // #50A14F - Forest green
            tool: Color::Rgb(166, 38, 164),     // #A626A4 - Purple
            brand: Color::Rgb(64, 120, 242),    // #4078F2 - Blue

            text_primary: Color::Rgb(56, 58, 66),      // #383A42
            text_secondary: Color::Rgb(105, 112, 119), // #697077

            background: Color::Rgb(250, 250, 250), // #FAFAFA
            background_alt: Color::Rgb(240, 240, 240), // #F0F0F0
            border: Color::Rgb(200, 200, 200),     // #C8C8C8

            is_dark: false,
        }
    }

    /// High contrast theme for accessibility
    pub fn high_contrast() -> Self {
        Self {
            name: "high-contrast",
            description: "High contrast theme for better visibility",

            primary: Color::Rgb(0, 255, 255), // Cyan
            accent: Color::Rgb(0, 255, 0),    // Bright green
            warning: Color::Rgb(255, 255, 0), // Bright yellow
            error: Color::Rgb(255, 0, 0),     // Bright red
            success: Color::Rgb(0, 255, 0),   // Bright green

            user: Color::Rgb(0, 255, 255),    // Cyan
            assistant: Color::Rgb(0, 255, 0), // Bright green
            tool: Color::Rgb(255, 0, 255),    // Magenta
            brand: Color::Rgb(0, 128, 255),   // Bright blue

            text_primary: Color::Rgb(255, 255, 255), // White
            text_secondary: Color::Rgb(192, 192, 192), // Light gray

            background: Color::Rgb(0, 0, 0),        // Pure black
            background_alt: Color::Rgb(32, 32, 32), // Very dark gray
            border: Color::Rgb(255, 255, 255),      // White

            is_dark: true,
        }
    }

    /// Solarized Dark theme
    pub fn solarized_dark() -> Self {
        Self {
            name: "solarized-dark",
            description: "Solarized dark color scheme",

            primary: Color::Rgb(38, 139, 210), // #268BD2 - Blue
            accent: Color::Rgb(133, 153, 0),   // #859900 - Green
            warning: Color::Rgb(181, 137, 0),  // #B58900 - Yellow
            error: Color::Rgb(220, 50, 47),    // #DC322F - Red
            success: Color::Rgb(133, 153, 0),  // #859900 - Green

            user: Color::Rgb(42, 161, 152),     // #2AA198 - Cyan
            assistant: Color::Rgb(133, 153, 0), // #859900 - Green
            tool: Color::Rgb(108, 113, 196),    // #6C71C4 - Violet
            brand: Color::Rgb(38, 139, 210),    // #268BD2 - Blue

            text_primary: Color::Rgb(131, 148, 150), // #839496 - Base0
            text_secondary: Color::Rgb(101, 123, 131), // #657B83 - Base00

            background: Color::Rgb(0, 43, 54), // #002B36 - Base03
            background_alt: Color::Rgb(7, 54, 66), // #073642 - Base02
            border: Color::Rgb(88, 110, 117),  // #586E75 - Base01

            is_dark: true,
        }
    }

    /// Solarized Light theme
    pub fn solarized_light() -> Self {
        Self {
            name: "solarized-light",
            description: "Solarized light color scheme",

            primary: Color::Rgb(38, 139, 210), // #268BD2 - Blue
            accent: Color::Rgb(133, 153, 0),   // #859900 - Green
            warning: Color::Rgb(181, 137, 0),  // #B58900 - Yellow
            error: Color::Rgb(220, 50, 47),    // #DC322F - Red
            success: Color::Rgb(133, 153, 0),  // #859900 - Green

            user: Color::Rgb(42, 161, 152),     // #2AA198 - Cyan
            assistant: Color::Rgb(133, 153, 0), // #859900 - Green
            tool: Color::Rgb(108, 113, 196),    // #6C71C4 - Violet
            brand: Color::Rgb(38, 139, 210),    // #268BD2 - Blue

            text_primary: Color::Rgb(101, 123, 131), // #657B83 - Base00
            text_secondary: Color::Rgb(131, 148, 150), // #839496 - Base0

            background: Color::Rgb(253, 246, 227), // #FDF6E3 - Base3
            background_alt: Color::Rgb(238, 232, 213), // #EEE8D5 - Base2
            border: Color::Rgb(147, 161, 161),     // #93A1A1 - Base1

            is_dark: false,
        }
    }

    /// Get a theme by name
    pub fn by_name(name: &str) -> Option<Self> {
        match name.to_lowercase().as_str() {
            "dark" | "default" => Some(Self::dark()),
            "light" => Some(Self::light()),
            "high-contrast" | "highcontrast" | "hc" => Some(Self::high_contrast()),
            "solarized-dark" | "solarized_dark" => Some(Self::solarized_dark()),
            "solarized-light" | "solarized_light" => Some(Self::solarized_light()),
            _ => None,
        }
    }

    /// List all available theme names
    pub fn available_themes() -> &'static [&'static str] {
        &[
            "dark",
            "light",
            "high-contrast",
            "solarized-dark",
            "solarized-light",
        ]
    }

    /// Try to detect if terminal has a dark or light background
    /// This is a best-effort detection based on environment variables
    pub fn detect_preferred() -> Self {
        // Check COLORFGBG environment variable (format: "fg;bg")
        // Common in xterm and compatible terminals
        if let Ok(colorfgbg) = std::env::var("COLORFGBG") {
            if let Some(bg) = colorfgbg.split(';').nth(1) {
                // Dark backgrounds typically have values 0-7
                // Light backgrounds typically have values 8-15 or higher
                if let Ok(bg_num) = bg.parse::<u8>() {
                    if bg_num >= 8 || bg_num == 7 {
                        return Self::light();
                    }
                }
            }
        }

        // Check terminal-specific environment variables
        if let Ok(term_program) = std::env::var("TERM_PROGRAM") {
            // iTerm2 reports its profile
            if term_program == "iTerm.app" {
                if let Ok(profile) = std::env::var("ITERM_PROFILE") {
                    if profile.to_lowercase().contains("light") {
                        return Self::light();
                    }
                }
            }
        }

        // Check for explicit dark/light mode setting
        if let Ok(color_scheme) = std::env::var("CAGENT_THEME") {
            if let Some(theme) = Self::by_name(&color_scheme) {
                return theme;
            }
        }

        // Check macOS appearance
        #[cfg(target_os = "macos")]
        {
            if let Ok(output) = std::process::Command::new("defaults")
                .args(["read", "-g", "AppleInterfaceStyle"])
                .output()
            {
                let stdout = String::from_utf8_lossy(&output.stdout);
                // If command succeeds and returns "Dark", we're in dark mode
                // If command fails (exits with error), we're in light mode (default)
                if !stdout.trim().eq_ignore_ascii_case("Dark") && output.status.success() {
                    return Self::light();
                }
            }
        }

        // Default to dark theme
        Self::dark()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_theme_by_name() {
        assert!(Theme::by_name("dark").is_some());
        assert!(Theme::by_name("light").is_some());
        assert!(Theme::by_name("high-contrast").is_some());
        assert!(Theme::by_name("nonexistent").is_none());
    }

    #[test]
    fn test_available_themes() {
        let themes = Theme::available_themes();
        assert!(themes.contains(&"dark"));
        assert!(themes.contains(&"light"));
    }
}
