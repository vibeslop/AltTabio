//! Menu bar item with the same commands as the Windows tray icon.

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSBezierPath, NSColor, NSFont, NSFontAttributeName, NSFontWeightSemibold,
    NSForegroundColorAttributeName, NSImage, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem,
    NSStringDrawing,
};
use objc2_foundation::{
    NSAttributedStringKey, NSDictionary, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize,
    NSString,
};
use std::rc::Rc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MenuAction {
    ShowSwitcher,
    OpenSettings,
    ShowAbout,
    /// Check for Updates, or Relaunch to Update once one is installed.
    Update,
    Quit,
}

pub struct MenuTargetIvars {
    handler: Rc<dyn Fn(MenuAction)>,
}

define_class!(
    // SAFETY:
    // - NSObject has no subclassing requirements.
    // - MenuTarget does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AltTabioMenuTarget"]
    #[ivars = MenuTargetIvars]
    pub struct MenuTarget;

    // SAFETY: NSObjectProtocol has no safety requirements.
    unsafe impl NSObjectProtocol for MenuTarget {}

    impl MenuTarget {
        // SAFETY: the action signatures match the target/action convention.
        #[unsafe(method(showSwitcher:))]
        fn show_switcher(&self, _sender: Option<&AnyObject>) {
            (self.ivars().handler)(MenuAction::ShowSwitcher);
        }

        #[unsafe(method(openSettings:))]
        fn open_settings(&self, _sender: Option<&AnyObject>) {
            (self.ivars().handler)(MenuAction::OpenSettings);
        }

        #[unsafe(method(showAbout:))]
        fn show_about(&self, _sender: Option<&AnyObject>) {
            (self.ivars().handler)(MenuAction::ShowAbout);
        }

        #[unsafe(method(update:))]
        fn update(&self, _sender: Option<&AnyObject>) {
            (self.ivars().handler)(MenuAction::Update);
        }

        #[unsafe(method(quit:))]
        fn quit(&self, _sender: Option<&AnyObject>) {
            (self.ivars().handler)(MenuAction::Quit);
        }
    }
);

impl MenuTarget {
    fn new(mtm: MainThreadMarker, handler: Rc<dyn Fn(MenuAction)>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(MenuTargetIvars { handler });
        unsafe {
            // SAFETY: init is NSObject's designated initializer.
            msg_send![super(this), init]
        }
    }
}

const CHECK_FOR_UPDATES: &str = "Check for Updates…";
const RELAUNCH_TO_UPDATE: &str = "Relaunch to Update";

pub struct StatusItem {
    _item: Retained<NSStatusItem>,
    _target: Retained<MenuTarget>,
    _menu: Retained<NSMenu>,
    update: Retained<NSMenuItem>,
}

impl StatusItem {
    /// Names the update command for whether an installed update waits for a relaunch.
    pub fn set_update_installed(&self, installed: bool) {
        let title = if installed {
            RELAUNCH_TO_UPDATE
        } else {
            CHECK_FOR_UPDATES
        };
        self.update.setTitle(&NSString::from_str(title));
    }
}

pub fn install(mtm: MainThreadMarker, handler: Rc<dyn Fn(MenuAction)>) -> StatusItem {
    let target = MenuTarget::new(mtm, handler);
    let item = NSStatusBar::systemStatusBar().statusItemWithLength(-1.0);
    if let Some(button) = item.button(mtm) {
        button.setImage(Some(&template_icon()));
        button.setToolTip(Some(&NSString::from_str(concat!(
            "AltTabio ",
            env!("CARGO_PKG_VERSION")
        ))));
    }
    let menu = NSMenu::new(mtm);
    menu.setAutoenablesItems(false);
    let version = menu_item(
        mtm,
        concat!("AltTabio ", env!("CARGO_PKG_VERSION")),
        None,
        "",
        &target,
    );
    version.setEnabled(false);
    menu.addItem(&version);
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    menu.addItem(&menu_item(
        mtm,
        "Show",
        Some(sel!(showSwitcher:)),
        "",
        &target,
    ));
    menu.addItem(&menu_item(
        mtm,
        "Settings…",
        Some(sel!(openSettings:)),
        ",",
        &target,
    ));
    menu.addItem(&menu_item(
        mtm,
        "About AltTabio",
        Some(sel!(showAbout:)),
        "",
        &target,
    ));
    let update = menu_item(mtm, CHECK_FOR_UPDATES, Some(sel!(update:)), "", &target);
    menu.addItem(&update);
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    menu.addItem(&menu_item(
        mtm,
        "Quit AltTabio",
        Some(sel!(quit:)),
        "q",
        &target,
    ));
    item.setMenu(Some(&menu));
    StatusItem {
        _item: item,
        _target: target,
        _menu: menu,
        update,
    }
}

fn menu_item(
    mtm: MainThreadMarker,
    title: &str,
    action: Option<objc2::runtime::Sel>,
    key: &str,
    target: &MenuTarget,
) -> Retained<NSMenuItem> {
    let item = unsafe {
        // SAFETY: every selector passed here exists on MenuTarget with a matching signature.
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            action,
            &NSString::from_str(key),
        )
    };
    unsafe {
        // SAFETY: the target is retained by the StatusItem for as long as the menu exists.
        item.setTarget(Some(target));
    }
    item
}

/// The `AltTabio` mark as a template glyph: the Tab key, outlined around its legend, the way the
/// menu bar shows the input source.
fn template_icon() -> Retained<NSImage> {
    const SIZE: NSSize = NSSize::new(25.0, 16.0);
    const STROKE: f64 = 1.25;
    let handler = RcBlock::new(|_rect: NSRect| -> Bool {
        let ink = NSColor::blackColor();
        ink.setStroke();
        let key = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
            NSRect::new(
                NSPoint::new(STROKE / 2.0, STROKE / 2.0),
                NSSize::new(SIZE.width - STROKE, SIZE.height - STROKE),
            ),
            4.0,
            4.0,
        );
        key.setLineWidth(STROKE);
        key.stroke();

        let font = unsafe {
            // SAFETY: the font weight constant is a static value exported by AppKit.
            NSFont::systemFontOfSize_weight(10.0, NSFontWeightSemibold)
        };
        let keys: [&NSAttributedStringKey; 2] = unsafe {
            // SAFETY: the attribute name constants are static strings exported by AppKit.
            [NSFontAttributeName, NSForegroundColorAttributeName]
        };
        let objects: [&AnyObject; 2] = [&font, &ink];
        let attributes = NSDictionary::from_slices(&keys, &objects);
        let legend = NSString::from_str("tab");
        let width = unsafe {
            // SAFETY: the attributes dictionary is live for the synchronous measurement.
            legend.sizeWithAttributes(Some(&attributes)).width
        };
        // "tab" has no descender, so its ink runs from the baseline up about the cap height;
        // centering that, rather than the line box, keeps the word in the middle of the key.
        let baseline = f64::midpoint(SIZE.height, font.capHeight());
        unsafe {
            // SAFETY: the drawing handler runs with the image's graphics context current.
            legend.drawAtPoint_withAttributes(
                NSPoint::new((SIZE.width - width) / 2.0, baseline - font.ascender()),
                Some(&attributes),
            );
        }
        Bool::YES
    });
    let image = NSImage::imageWithSize_flipped_drawingHandler(SIZE, true, &handler);
    image.setTemplate(true);
    image
}
