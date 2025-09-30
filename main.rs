use anyhow::Result;
use glib::clone;
use glib::object::ObjectExt;   // for .set_property() / .property()
use glib::ControlFlow;         // ControlFlow::Continue for bus watch
use gtk4 as gtk;
use gtk::gdk;
use gtk::prelude::*;
use gstreamer as gst;
use gstreamer::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScaleMode { Fit, Stretch, OneToOne }

fn main() -> Result<()> {
    gtk::init()?;
    gst::init()?;

    let app = gtk::Application::builder()
        .application_id("dev.iam.rustvideoplayer")
        .build();

    app.connect_activate(|app| {
        build_ui(app).expect("failed to build UI");
    });

    app.run();
    Ok(())
}

fn build_ui(app: &gtk::Application) -> Result<()> {
    let win = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Rust MP4 Player")
        .default_width(1024)
        .default_height(640)
        .build();

    // Header bar + controls
    let header = gtk::HeaderBar::new();
    let open_btn = gtk::Button::with_label("Open…");
    let play_btn = gtk::Button::with_label("Play");
    let pause_btn = gtk::Button::with_label("Pause");
    let stop_btn = gtk::Button::with_label("Stop");
    let fit_btn = gtk::ToggleButton::with_label("Fit");
    let stretch_btn = gtk::ToggleButton::with_label("Stretch");
    let one_btn = gtk::ToggleButton::with_label("1:1");
    let full_btn = gtk::ToggleButton::with_label("Fullscreen");

    header.pack_start(&open_btn);
    header.pack_start(&play_btn);
    header.pack_start(&pause_btn);
    header.pack_start(&stop_btn);
    header.pack_end(&full_btn);
    header.pack_end(&one_btn);
    header.pack_end(&stretch_btn);
    header.pack_end(&fit_btn);

    // Picture inside a ScrolledWindow so 1:1 can scroll if too big
    let picture = gtk::Picture::builder()
        .can_shrink(true)            // we'll toggle per mode
        .keep_aspect_ratio(true)     // default Fit keeps aspect
        .hexpand(true)
        .vexpand(true)
        .build();

    let scroller = gtk::ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .build();
    scroller.set_child(Some(&picture));

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
    vbox.append(&header);
    vbox.append(&scroller);
    win.set_child(Some(&vbox));

    // --- GStreamer: playbin + gtk4paintablesink
    let playbin = gst::ElementFactory::make("playbin")
        .build()
        .expect("missing playbin (install gstreamer1.0*)");

    let sink = gst::ElementFactory::make("gtk4paintablesink")
        .build()
        .expect("gtk4paintablesink missing (install gstreamer1.0-gtk4)");

    // glib::ObjectExt::set_property returns (), so don't use ?/.ok()
    playbin.set_property("video-sink", &sink);

    // Get GdkPaintable from sink and set on Picture
    let paintable: gtk::gdk::Paintable = sink.property("paintable");
    picture.set_paintable(Some(&paintable));

    // Bus messages
    let pb = playbin.clone();
    let win_weak = win.downgrade();
    let _watch = playbin
        .bus()
        .expect("no bus")
        .add_watch_local(move |_, msg| {
            use gst::MessageView;
            match msg.view() {
                MessageView::Eos(..) => { let _ = pb.set_state(gst::State::Ready); }
                MessageView::Error(err) => {
                    if let Some(win) = win_weak.upgrade() {
                        let d = gtk::MessageDialog::builder()
                            .transient_for(&win)
                            .modal(true)
                            .message_type(gtk::MessageType::Error)
                            .text("Playback error")
                            .secondary_text(format!("{} ({:?})", err.error(), err.debug()))
                            .build();
                        d.add_button("Close", gtk::ResponseType::Close);
                        d.connect_response(|d, _| d.close());
                        d.show();
                    }
                    let _ = pb.set_state(gst::State::Ready);
                }
                _ => {}
            }
            ControlFlow::Continue
        })?;

    // Open…
    open_btn.connect_clicked(clone!(@weak win, @strong playbin => move |_| {
        let dialog = gtk::FileChooserNative::builder()
            .title("Open Video")
            .transient_for(&win)
            .action(gtk::FileChooserAction::Open)
            .accept_label("Open")
            .build();

        let filter = gtk::FileFilter::new();
        filter.add_pattern("*.mp4");              // works on gtk4 v0.8
        filter.add_mime_type("video/mp4");
        filter.set_name(Some("MP4 video"));
        dialog.add_filter(&filter);

        dialog.connect_response(clone!(@strong playbin => move |dlg, resp| {
            if resp == gtk::ResponseType::Accept {
                if let Some(file) = dlg.file() {
                    let uri = file.uri();        // GString (not Option) on gtk4 v0.8
                    playbin.set_property("uri", &uri);
                    let _ = playbin.set_state(gst::State::Playing);
                }
            }
            dlg.destroy();
        }));
        dialog.show();
    }));

    // Transport
    play_btn.connect_clicked(clone!(@strong playbin => move |_| { let _ = playbin.set_state(gst::State::Playing); }));
    pause_btn.connect_clicked(clone!(@strong playbin => move |_| { let _ = playbin.set_state(gst::State::Paused); }));
    stop_btn.connect_clicked(clone!(@strong playbin => move |_| { let _ = playbin.set_state(gst::State::Ready); }));

    // Scale modes without ContentFit (older GTK4):
    // - Fit:     keep aspect, fill available space (shrink/expand)
    // - Stretch: ignore aspect, fill window
    // - 1:1:     keep aspect at natural size (no upscale); scroller handles overflow
    let set_mode = {
        let picture = picture.clone();
        move |m: ScaleMode| {
            match m {
                ScaleMode::Fit => {
                    picture.set_keep_aspect_ratio(true);
                    picture.set_can_shrink(true);
                    picture.set_halign(gtk::Align::Fill);
                    picture.set_valign(gtk::Align::Fill);
                }
                ScaleMode::Stretch => {
                    picture.set_keep_aspect_ratio(false);
                    picture.set_can_shrink(true);
                    picture.set_halign(gtk::Align::Fill);
                    picture.set_valign(gtk::Align::Fill);
                }
                ScaleMode::OneToOne => {
                    // Request natural size, avoid scaling; center it.
                    picture.set_keep_aspect_ratio(true);
                    picture.set_can_shrink(false);
                    picture.set_halign(gtk::Align::Center);
                    picture.set_valign(gtk::Align::Center);
                }
            }
        }
    };
    set_mode(ScaleMode::Fit);

    // Exclusive toggle behaviour (manual = portable)
    {
        let s_fit = fit_btn.clone();
        let s_stretch = stretch_btn.clone();
        let s_one = one_btn.clone();

        fit_btn.connect_toggled(clone!(@strong s_stretch, @strong s_one, @strong set_mode => move |b| {
            if b.is_active() {
                s_stretch.set_active(false);
                s_one.set_active(false);
                set_mode(ScaleMode::Fit);
            }
        }));
        stretch_btn.connect_toggled(clone!(@strong s_fit, @strong s_one, @strong set_mode => move |b| {
            if b.is_active() {
                s_fit.set_active(false);
                s_one.set_active(false);
                set_mode(ScaleMode::Stretch);
            }
        }));
        one_btn.connect_toggled(clone!(@strong s_fit, @strong s_stretch, @strong set_mode => move |b| {
            if b.is_active() {
                s_fit.set_active(false);
                s_stretch.set_active(false);
                set_mode(ScaleMode::OneToOne);
            }
        }));

        fit_btn.set_active(true);
    }

    // Fullscreen + keyboard shortcuts
    full_btn.connect_toggled(clone!(@weak win => move |b| if b.is_active(){ win.fullscreen(); } else { win.unfullscreen(); }));

    win.add_controller({
        let kb = gtk::EventControllerKey::new();
        kb.connect_key_pressed(clone!(@weak win, @strong playbin, @weak fit_btn, @weak stretch_btn, @weak one_btn, @weak full_btn
            => @default-return glib::Propagation::Proceed, move |_c, key, _code, _state| {
            match key {
                gdk::Key::space => {
                    if playbin.current_state() == gst::State::Playing {
                        let _ = playbin.set_state(gst::State::Paused);
                    } else {
                        let _ = playbin.set_state(gst::State::Playing);
                    }
                    glib::Propagation::Stop
                }
                gdk::Key::F | gdk::Key::f => {
                    full_btn.set_active(!full_btn.is_active());
                    glib::Propagation::Stop
                }
                gdk::Key::_1 => { fit_btn.set_active(true); glib::Propagation::Stop }
                gdk::Key::_2 => { stretch_btn.set_active(true); glib::Propagation::Stop }
                gdk::Key::_3 => { one_btn.set_active(true); glib::Propagation::Stop }
                _ => glib::Propagation::Proceed
            }
        }));
        kb
    });

    win.present();
    Ok(())
}

