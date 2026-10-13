// SPDX-License-Identifier: AGPL-3.0-only

//! The device name hjv-api shows for an account key (design v3 §4-1 step 7, §1-1): the new-login
//! notice and "로그인한 기기" name a device by what the server makes of the login, not by text
//! the device chose. The app's `deviceInfo.name` is the model code on an iPhone
//! (`utsname.machine`, such as "iPhone15,2"), "<brand>-<model>" on Android and the host name on a
//! PC (client research notes A#85); `deviceInfo.os` is the platform. This table turns a known
//! model code into its product name and anything else into a plain name for the platform, so
//! that a device's own words appear only where the app puts them, in parentheses (`app_name`).
//!
//! The table lives on the server, so a new model is a change here and a new hjv-api, without a
//! new app.

/// iPhone model codes and their product names.
const IPHONES: &[(&str, &str)] = &[
    ("iPhone8,1", "iPhone 6s"),
    ("iPhone8,2", "iPhone 6s Plus"),
    ("iPhone8,4", "iPhone SE"),
    ("iPhone9,1", "iPhone 7"),
    ("iPhone9,3", "iPhone 7"),
    ("iPhone9,2", "iPhone 7 Plus"),
    ("iPhone9,4", "iPhone 7 Plus"),
    ("iPhone10,1", "iPhone 8"),
    ("iPhone10,4", "iPhone 8"),
    ("iPhone10,2", "iPhone 8 Plus"),
    ("iPhone10,5", "iPhone 8 Plus"),
    ("iPhone10,3", "iPhone X"),
    ("iPhone10,6", "iPhone X"),
    ("iPhone11,2", "iPhone XS"),
    ("iPhone11,4", "iPhone XS Max"),
    ("iPhone11,6", "iPhone XS Max"),
    ("iPhone11,8", "iPhone XR"),
    ("iPhone12,1", "iPhone 11"),
    ("iPhone12,3", "iPhone 11 Pro"),
    ("iPhone12,5", "iPhone 11 Pro Max"),
    ("iPhone12,8", "iPhone SE (2nd generation)"),
    ("iPhone13,1", "iPhone 12 mini"),
    ("iPhone13,2", "iPhone 12"),
    ("iPhone13,3", "iPhone 12 Pro"),
    ("iPhone13,4", "iPhone 12 Pro Max"),
    ("iPhone14,2", "iPhone 13 Pro"),
    ("iPhone14,3", "iPhone 13 Pro Max"),
    ("iPhone14,4", "iPhone 13 mini"),
    ("iPhone14,5", "iPhone 13"),
    ("iPhone14,6", "iPhone SE (3rd generation)"),
    ("iPhone14,7", "iPhone 14"),
    ("iPhone14,8", "iPhone 14 Plus"),
    ("iPhone15,2", "iPhone 14 Pro"),
    ("iPhone15,3", "iPhone 14 Pro Max"),
    ("iPhone15,4", "iPhone 15"),
    ("iPhone15,5", "iPhone 15 Plus"),
    ("iPhone16,1", "iPhone 15 Pro"),
    ("iPhone16,2", "iPhone 15 Pro Max"),
    ("iPhone17,1", "iPhone 16 Pro"),
    ("iPhone17,2", "iPhone 16 Pro Max"),
    ("iPhone17,3", "iPhone 16"),
    ("iPhone17,4", "iPhone 16 Plus"),
    ("iPhone17,5", "iPhone 16e"),
    ("iPhone18,1", "iPhone 17 Pro"),
    ("iPhone18,2", "iPhone 17 Pro Max"),
    ("iPhone18,3", "iPhone 17"),
    ("iPhone18,4", "iPhone Air"),
];

/// The platforms the app reports (`std::env::consts::OS`), as hjv-api stores them; anything
/// else is "other".
pub fn platform(os: &str) -> &'static str {
    match os.trim().to_ascii_lowercase().as_str() {
        "ios" => "ios",
        "android" => "android",
        "windows" => "windows",
        "macos" => "macos",
        "linux" => "linux",
        _ => "other",
    }
}

/// The name hjv-api gives a device of `platform` (as [`platform`] returns it) that reported
/// `reported` as its `deviceInfo.name`.
pub fn model_name(platform: &str, reported: &str) -> &'static str {
    let reported = reported.trim();
    match platform {
        "ios" => {
            if let Some((_, name)) = IPHONES.iter().find(|(code, _)| *code == reported) {
                name
            } else if reported.starts_with("iPhone") {
                "iPhone"
            } else if reported.starts_with("iPad") {
                "iPad"
            } else if reported.starts_with("iPod") {
                "iPod touch"
            } else {
                "iOS"
            }
        }
        "android" => {
            let brand = reported
                .split('-')
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            match brand.as_str() {
                "samsung" => "Galaxy",
                "google" => "Pixel",
                _ => "Android",
            }
        }
        "windows" => "Windows PC",
        "macos" => "Mac",
        "linux" => "Linux PC",
        _ => "Unknown device",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(model_name("ios", "iPhone15,2"), "iPhone 14 Pro");
        assert_eq!(model_name("ios", " iPhone17,5 "), "iPhone 16e");
        // a model the table does not know yet, and a device's own words, stay generic
        assert_eq!(model_name("ios", "iPhone99,9"), "iPhone");
        assert_eq!(model_name("ios", "iPad13,4"), "iPad");
        assert_eq!(model_name("ios", "Hong's phone"), "iOS");
        assert_eq!(model_name("android", "samsung-SM-S918N"), "Galaxy");
        assert_eq!(model_name("android", "Google-Pixel 8"), "Pixel");
        assert_eq!(model_name("android", "LGE-LM-V500N"), "Android");
        assert_eq!(model_name("windows", "LAB-PC"), "Windows PC");
        assert_eq!(model_name("other", "x"), "Unknown device");
        assert_eq!(platform(" Windows "), "windows");
        assert_eq!(platform("ios"), "ios");
        assert_eq!(platform("freebsd"), "other");
        // every code once
        for (i, (code, _)) in IPHONES.iter().enumerate() {
            assert!(!IPHONES[i + 1..].iter().any(|(c, _)| c == code), "{code}");
        }
    }
}
