//! The open panel on macOS. It sets no allowed file types, so AppKit greys nothing out by type; a
//! delegate enables each file by comparing its extension with the filter picked in the "Show" menu
//! below the file list (Filter::accepts, any letter case). The panel opens as a sheet on the window.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use block2::RcBlock;
use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSModalResponse, NSModalResponseOK, NSOpenPanel, NSOpenSavePanelDelegate, NSPopUpButton,
    NSStackView, NSTextField, NSView, NSWindow,
};
use objc2_foundation::{NSArray, NSEdgeInsets, NSRect, NSString, NSURL};
use tauri::Window;

use super::Filter;

type Picked = Result<Vec<PathBuf>, String>;

/// Runs the panel on the main thread and waits here for the user to close it.
pub fn pick(window: &Window, title: &str, filters: Vec<Filter>, multiple: bool) -> Picked {
    let (done, picked) = mpsc::channel::<Picked>();
    let parent = window.clone();
    let title = title.to_owned();
    window
        .run_on_main_thread(move || {
            let mtm = MainThreadMarker::new().expect("run_on_main_thread runs on the main thread");
            let ns_window = match parent.ns_window() {
                // SAFETY: Tauri's NSWindow; on the main thread nothing can close it while this runs.
                Ok(ptr) => unsafe { Retained::retain(ptr.cast::<NSWindow>()) },
                Err(_) => None,
            };
            match ns_window {
                Some(ns_window) => show(mtm, &ns_window, &title, filters, multiple, done),
                None => {
                    let _ = done.send(Err(
                        "Could not show the file dialog: the window is gone.".into()
                    ));
                }
            }
        })
        .map_err(|e| format!("Could not show the file dialog ({e})."))?;
    // The sender is dropped without a send only if the app quits first: treat that as cancelled.
    picked.recv().unwrap_or(Ok(Vec::new()))
}

fn show(
    mtm: MainThreadMarker,
    parent: &NSWindow,
    title: &str,
    filters: Vec<Filter>,
    multiple: bool,
    done: mpsc::Sender<Picked>,
) {
    let panel = NSOpenPanel::openPanel(mtm);
    panel.setCanChooseFiles(true);
    panel.setCanChooseDirectories(false);
    panel.setAllowsMultipleSelection(multiple);
    // A sheet has no title bar; the message line above the file list shows the title instead.
    panel.setMessage(Some(&NSString::from_str(title)));

    let names: Vec<Retained<NSString>> = filters
        .iter()
        .map(|f| NSString::from_str(&f.name))
        .collect();
    let delegate = FilterDelegate::new(mtm, filters, &panel);
    // SAFETY: the panel holds its delegate weakly; the completion handler keeps it alive until the
    // panel closes.
    unsafe { panel.setDelegate(Some(ProtocolObject::from_ref(&*delegate))) };
    panel.setAccessoryView(Some(&show_menu(mtm, &names, &delegate)));
    panel.setAccessoryViewDisclosed(true);

    let open = panel.clone();
    let handler = RcBlock::new(move |response: NSModalResponse| {
        let _delegate = &delegate;
        let picked = if response == NSModalResponseOK {
            open.URLs()
                .iter()
                .filter_map(|url| url.path())
                .map(|path| PathBuf::from(path.to_string()))
                .collect()
        } else {
            Vec::new()
        };
        let _ = done.send(Ok(picked));
    });
    panel.beginSheetModalForWindow_completionHandler(parent, &handler);
}

/// "Show: [All supported files ⌄]", below the file list.
fn show_menu(
    mtm: MainThreadMarker,
    names: &[Retained<NSString>],
    delegate: &FilterDelegate,
) -> Retained<NSView> {
    let menu =
        NSPopUpButton::initWithFrame_pullsDown(NSPopUpButton::alloc(mtm), NSRect::ZERO, false);
    menu.addItemsWithTitles(&NSArray::from_retained_slice(names));
    menu.selectItemAtIndex(0);
    // SAFETY: the delegate implements filterChanged: and lives as long as the panel (see show).
    unsafe {
        menu.setTarget(Some(delegate));
        menu.setAction(Some(sel!(filterChanged:)));
    }
    let label = NSTextField::labelWithString(&NSString::from_str("Show:"), mtm);
    let views: [&NSView; 2] = [&label, &menu];
    let row = NSStackView::stackViewWithViews(&NSArray::from_slice(&views), mtm);
    row.setEdgeInsets(NSEdgeInsets {
        top: 8.0,
        left: 20.0,
        bottom: 8.0,
        right: 20.0,
    });
    row.setFrameSize(row.fittingSize());
    Retained::into_super(row)
}

struct Ivars {
    filters: Vec<Filter>,
    /// The entry picked in the Show menu.
    selected: Cell<usize>,
    panel: Weak<NSOpenPanel>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and FilterDelegate does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "Tricky3dOpenPanelFilter"]
    #[ivars = Ivars]
    struct FilterDelegate;

    unsafe impl NSObjectProtocol for FilterDelegate {}

    unsafe impl NSOpenSavePanelDelegate for FilterDelegate {
        #[unsafe(method(panel:shouldEnableURL:))]
        fn panel_should_enable_url(&self, _sender: &AnyObject, url: &NSURL) -> bool {
            // (No early `return` here: define_class! turns the result into an Objective-C BOOL.)
            url.path().is_some_and(|path| self.enables(&PathBuf::from(path.to_string())))
        }
    }

    impl FilterDelegate {
        /// The Show menu's action: switch filters and re-check the files on screen.
        #[unsafe(method(filterChanged:))]
        fn filter_changed(&self, sender: &NSPopUpButton) {
            let index = usize::try_from(sender.indexOfSelectedItem()).unwrap_or(0);
            self.ivars().selected.set(index);
            if let Some(panel) = self.ivars().panel.load() {
                panel.validateVisibleColumns();
            }
        }
    }
);

impl FilterDelegate {
    fn enables(&self, path: &Path) -> bool {
        let ivars = self.ivars();
        let shown = ivars
            .filters
            .get(ivars.selected.get())
            .is_none_or(|f| f.accepts(path));
        // Folders stay enabled so they can be opened; only names the filter does not show are looked up.
        shown || path.is_dir()
    }

    fn new(
        mtm: MainThreadMarker,
        filters: Vec<Filter>,
        panel: &Retained<NSOpenPanel>,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            filters,
            selected: Cell::new(0),
            panel: Weak::from_retained(panel),
        });
        // SAFETY: NSObject's init.
        unsafe { msg_send![super(this), init] }
    }
}
