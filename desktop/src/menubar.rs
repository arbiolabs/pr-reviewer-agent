//! Where the app lives once its window closes: a menu bar item that opens it again or quits it, an optional Dock
//! icon, and a login item that starts it in the background.

use std::path::{Path, PathBuf};

/// The LaunchAgent that opens the app at login. Its plist being there is the whole setting.
const LOGIN_LABEL: &str = "io.github.diego-arbio-dev.pr-review.app";

/// Started by the login item: the window opens hidden, so login puts the app in the menu bar and nothing in front.
pub const BACKGROUND_FLAG: &str = "--background";

fn login_plist() -> PathBuf {
    crate::reviewer::home().join(format!("Library/LaunchAgents/{LOGIN_LABEL}.plist"))
}

pub fn opens_at_login() -> bool {
    login_plist().exists()
}

/// Writes or removes the login item. launchd reads the folder at the next login, so nothing is loaded now.
pub fn set_open_at_login(on: bool) -> Result<(), String> {
    let path = login_plist();
    let at = |e: std::io::Error| format!("{}: {e}", path.display());
    if !on {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(at(e)),
            _ => Ok(()),
        };
    }
    let exe = std::env::current_exe().map_err(|e| format!("can't find the app: {e}"))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(at)?;
    }
    std::fs::write(&path, login_agent(&exe)).map_err(at)
}

/// The plist: `open` on the app bundle when the binary is in one, so the app starts as macOS starts apps, else the
/// binary itself.
fn login_agent(exe: &Path) -> String {
    let args: Vec<String> = match bundle(exe) {
        Some(app) => {
            vec![
                "/usr/bin/open".into(),
                "-a".into(),
                app.display().to_string(),
                "--args".into(),
                BACKGROUND_FLAG.into(),
            ]
        }
        None => vec![exe.display().to_string(), BACKGROUND_FLAG.into()],
    };
    let args: String = args.iter().map(|a| format!("\n        <string>{}</string>", xml(a))).collect();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LOGIN_LABEL}</string>
    <key>ProgramArguments</key>
    <array>{args}
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>LimitLoadToSessionType</key>
    <string>Aqua</string>
</dict>
</plist>
"#
    )
}

/// The `.app` the binary runs from, when it runs from one.
pub fn bundle(exe: &Path) -> Option<&Path> {
    exe.ancestors().nth(3).filter(|b| b.extension().is_some_and(|e| e == "app"))
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Brings the window back: out of a hidden app, off the Dock if minimized, and in front.
#[cfg(target_os = "macos")]
fn show(mtm: objc2::MainThreadMarker) {
    let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
    app.unhide(None);
    for w in app.windows() {
        if w.isMiniaturized() {
            w.deminiaturize(None);
        }
        w.makeKeyAndOrderFront(None);
    }
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
}

#[cfg(target_os = "macos")]
mod tray {
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject};
    use objc2::{AllocAnyThread, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
    use objc2_app_kit::{NSApplication, NSImage, NSMenu, NSMenuItem, NSStatusBar, NSVariableStatusItemLength};
    use objc2_foundation::{NSData, NSSize, NSString};

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "PRReviewMenuBarTarget"]
        struct Target;

        impl Target {
            #[unsafe(method(open:))]
            fn open(&self, _: Option<&AnyObject>) {
                super::show(self.mtm());
            }

            #[unsafe(method(quit:))]
            fn quit(&self, _: Option<&AnyObject>) {
                NSApplication::sharedApplication(self.mtm()).terminate(None);
            }
        }
    );

    /// The app's mark as a template image, so macOS tints it for a light or dark menu bar.
    fn mark() -> Option<Retained<NSImage>> {
        let data = NSData::with_bytes(include_bytes!("../assets/mark.svg"));
        let image = NSImage::initWithData(NSImage::alloc(), &data)?;
        image.setSize(NSSize::new(18., 18.));
        image.setTemplate(true);
        Some(image)
    }

    /// Puts the item in the menu bar for the life of the process.
    pub fn install() {
        let Some(mtm) = MainThreadMarker::new() else { return };
        let target: Retained<Target> = unsafe { msg_send![Target::alloc(mtm), init] };
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        if let Some(button) = item.button(mtm) {
            match mark() {
                Some(image) => button.setImage(Some(&image)),
                None => button.setTitle(&NSString::from_str("PR")),
            }
            button.setToolTip(Some(&NSString::from_str(crate::ui::APP_NAME)));
        }
        let menu = NSMenu::new(mtm);
        let entry = |title: String, action, key: &str| {
            let title = NSString::from_str(&title);
            let key = NSString::from_str(key);
            let entry =
                unsafe { NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm), &title, action, &key) };
            unsafe { entry.setTarget(Some(&target)) };
            entry
        };
        menu.addItem(&entry(format!("Open {}", crate::ui::APP_NAME), Some(sel!(open:)), ""));
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        menu.addItem(&entry(format!("Quit {}", crate::ui::APP_NAME), Some(sel!(quit:)), "q"));
        item.setMenu(Some(&menu));
        // An item leaves the menu bar when released, and a menu item holds its target weakly: both live as long as
        // the process.
        std::mem::forget(item);
        std::mem::forget(target);
    }
}

#[cfg(target_os = "macos")]
pub use tray::install;

#[cfg(not(target_os = "macos"))]
pub fn install() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_login_item_opens_the_bundle_in_the_background() {
        let plist =
            login_agent(Path::new("/Users/me/Applications/PR Reviewer & Co.app/Contents/MacOS/pr-review-desktop"));
        assert!(plist.contains(
            "<string>/usr/bin/open</string>\n        <string>-a</string>\n        \
             <string>/Users/me/Applications/PR Reviewer &amp; Co.app</string>\n        <string>--args</string>\n        \
             <string>--background</string>"
        ));
        assert!(plist.contains("<key>RunAtLoad</key>\n    <true/>"));

        let bare = login_agent(Path::new("/src/desktop/target/release/pr-review-desktop"));
        assert!(bare.contains("<array>\n        <string>/src/desktop/target/release/pr-review-desktop</string>"));
        assert!(!bare.contains("/usr/bin/open"));
    }
}
