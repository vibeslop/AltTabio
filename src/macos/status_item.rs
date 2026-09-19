//! Menu bar item with the same commands as the Windows tray icon.

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSBezierPath, NSColor, NSImage, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};
use std::rc::Rc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MenuAction {
    ShowSwitcher,
    OpenSettings,
    ShowAbout,
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

pub struct StatusItem {
    _item: Retained<NSStatusItem>,
    _target: Retained<MenuTarget>,
    _menu: Retained<NSMenu>,
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

/// The `AltTabio` mark as a template glyph: two overlapping window outlines.
fn template_icon() -> Retained<NSImage> {
    let handler = RcBlock::new(|_rect: NSRect| -> Bool {
        NSColor::blackColor().setStroke();
        let back = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
            NSRect::new(NSPoint::new(1.5, 5.5), NSSize::new(10.0, 8.0)),
            2.0,
            2.0,
        );
        back.setLineWidth(1.5);
        back.stroke();
        NSColor::blackColor().setFill();
        let front = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
            NSRect::new(NSPoint::new(6.5, 1.5), NSSize::new(10.0, 8.0)),
            2.0,
            2.0,
        );
        front.fill();
        Bool::YES
    });
    let image =
        NSImage::imageWithSize_flipped_drawingHandler(NSSize::new(18.0, 15.0), true, &handler);
    image.setTemplate(true);
    image
}
