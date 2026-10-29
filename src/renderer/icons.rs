use super::RenderOptions;
use crate::task_icon::TaskIcons;
use alttabio::overlay_layout::for_compact_list;
use alttabio::switcher::Switcher;
use std::collections::HashMap;
use std::fmt;
use std::rc::Weak;
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{
    D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, ID2D1Bitmap, ID2D1HwndRenderTarget,
};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory,
    WICConvertBitmapSource,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::UI::WindowsAndMessaging::HICON;
use windows::core::{Error, Result};

pub(super) struct IconPassFailure {
    failed: usize,
    first: Error,
}

impl fmt::Display for IconPassFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Could not prepare {} task icon(s): {}",
            self.failed, self.first
        )
    }
}

/// Device-dependent copies are bounded by the current owned-icon snapshot and render target.
pub(super) struct IconBitmaps {
    factory: IWICImagingFactory,
    owner: Weak<()>,
    bitmaps: HashMap<isize, Result<ID2D1Bitmap>>,
}

impl IconBitmaps {
    pub(super) fn new() -> Result<Self> {
        // SAFETY: Renderer is created on the UI thread with its owning COM apartment initialized.
        let factory =
            unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }?;
        Ok(Self {
            factory,
            owner: Weak::new(),
            bitmaps: HashMap::new(),
        })
    }

    pub(super) fn clear(&mut self) {
        self.bitmaps.clear();
    }

    pub(super) fn for_snapshot(&mut self, icons: &TaskIcons) {
        let owner = icons.identity();
        if !self.owner.ptr_eq(&owner) {
            self.clear();
            self.owner = owner;
        }
    }

    pub(super) fn draw(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        switcher: &Switcher,
        options: RenderOptions,
        height: f32,
    ) -> Option<IconPassFailure> {
        let layout = for_compact_list(options.compact_list);
        let rows = layout.visible_row_count(height);
        let start = switcher.visible_range(rows).start;
        let mut failure: Option<IconPassFailure> = None;
        for (position, task) in switcher.positioned_visible_tasks().skip(start).take(rows) {
            if task.icon_handle == 0 {
                continue;
            }
            let bitmap = self.bitmaps.entry(task.icon_handle).or_insert_with(|| {
                load_bitmap(&self.factory, target, HICON(task.icon_handle as *mut _))
            });
            match bitmap {
                Ok(bitmap) => {
                    let bounds = layout.icon_bounds(
                        position - 1 - start,
                        options.show_numbers,
                        options.large_icons,
                    );
                    let destination = D2D_RECT_F {
                        left: bounds.left,
                        top: bounds.top,
                        right: bounds.right,
                        bottom: bounds.bottom,
                    };
                    // SAFETY: this target created the bitmap; the caller holds BeginDraw/EndDraw.
                    unsafe {
                        target.DrawBitmap(
                            &*bitmap,
                            Some(&raw const destination),
                            1.0,
                            D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                            None,
                        );
                    }
                }
                Err(error) => {
                    if let Some(failure) = &mut failure {
                        failure.failed += 1;
                    } else {
                        failure = Some(IconPassFailure {
                            failed: 1,
                            first: error.clone(),
                        });
                    }
                }
            }
        }
        failure
    }
}

fn load_bitmap(
    factory: &IWICImagingFactory,
    target: &ID2D1HwndRenderTarget,
    icon: HICON,
) -> Result<ID2D1Bitmap> {
    // SAFETY: the task snapshot owns the source HICON for this draw. WIC and Direct2D copy its
    // pixels; neither takes ownership of the handle. All COM objects stay on this UI thread.
    unsafe {
        let source = factory.CreateBitmapFromHICON(icon)?;
        let pixels = WICConvertBitmapSource(&GUID_WICPixelFormat32bppPBGRA, &source)?;
        target.CreateBitmapFromWicBitmap(&pixels, None)
    }
}
