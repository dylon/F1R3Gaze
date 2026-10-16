//! Launch Services URL delivery for the macOS app bundle.
//!
//! Winit leaves `NSApplicationDelegate` to the application. AppKit calls
//! `application:openURLs:` for a registered URL scheme both when Launch
//! Services starts the app and when it sends a URL to a running instance.
//! The delegate queues validated URLs on the main thread and wakes winit;
//! `ChromeApplication` drains the queue before polling the browser document.

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSApplication, NSApplicationDelegate};
use objc2_foundation::{NSArray, NSObject, NSObjectProtocol, NSURL};
use std::cell::RefCell;
use std::collections::VecDeque;
use winit::event_loop::EventLoopProxy;

#[derive(Default)]
struct Delivery {
    pending: VecDeque<String>,
    proxy: Option<EventLoopProxy>,
}

thread_local! {
    static DELIVERY: RefCell<Delivery> = RefCell::new(Delivery::default());
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "F1R3GazeApplicationDelegate"]
    struct AppDelegate;

    unsafe impl NSObjectProtocol for AppDelegate {}

    unsafe impl NSApplicationDelegate for AppDelegate {
        #[unsafe(method(application:openURLs:))]
        #[allow(non_snake_case)] // The Objective-C protocol fixes this selector spelling.
        fn application_openURLs(&self, _application: &NSApplication, urls: &NSArray<NSURL>) {
            for url in urls.iter() {
                if let Some(absolute) = url.absoluteString() {
                    queue(&absolute.to_string());
                }
            }
        }
    }
);

impl AppDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        unsafe { msg_send![super(Self::alloc(mtm).set_ivars(())), init] }
    }
}

/// Keep the delegate alive until the event loop returns.
pub(crate) struct Registration {
    _delegate: Retained<AppDelegate>,
}

/// Install after `EventLoop::builder().build()`, as winit-appkit requires.
pub(crate) fn install(proxy: EventLoopProxy) -> Result<Registration, String> {
    let mtm = MainThreadMarker::new().ok_or("the macOS URL delegate requires the main thread")?;
    DELIVERY.with(|delivery| {
        let mut delivery = delivery.borrow_mut();
        if delivery.proxy.is_some() {
            return Err("the macOS URL delegate is already installed".to_string());
        }
        delivery.proxy = Some(proxy);
        Ok(())
    })?;
    let delegate = AppDelegate::new(mtm);
    let app = NSApplication::sharedApplication(mtm);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    Ok(Registration {
        _delegate: delegate,
    })
}

impl Drop for Registration {
    fn drop(&mut self) {
        // `Registration` is main-thread-only because its retained delegate is.
        let mtm = MainThreadMarker::new().expect("the AppKit delegate stays on the main thread");
        NSApplication::sharedApplication(mtm).setDelegate(None);
        DELIVERY.with(|delivery| {
            let mut delivery = delivery.borrow_mut();
            delivery.proxy = None;
            delivery.pending.clear();
        });
    }
}

fn queue(raw: &str) {
    if let Some(url) = crate::external_url::accepted_url(raw) {
        DELIVERY.with(|delivery| {
            let mut delivery = delivery.borrow_mut();
            delivery.pending.push_back(url);
            if let Some(proxy) = &delivery.proxy {
                proxy.wake_up();
            }
        });
    }
}

pub(crate) fn take_pending() -> Vec<String> {
    DELIVERY.with(|delivery| delivery.borrow_mut().pending.drain(..).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_valid_registered_urls_are_delivered_in_order() {
        let content = format!("f1r3h://blake2b-256/{}", "a".repeat(64));
        queue("f1r3://ABCD/project");
        queue("https://example.org/");
        queue("f1r3://not-hex/project");
        queue("f1r3h://blake2b-256/not-a-hash");
        queue(&content);
        assert_eq!(take_pending(), ["f1r3://ABCD/project", content.as_str()]);
        assert!(take_pending().is_empty());
    }
}
