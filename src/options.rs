//! App picker GUI.
//!
//! This also includes a license text viewer. The license text viewer is needed
//! on Android, where the command-line way to view license text doesn't exist.

use crate::bundle::Bundle;
use crate::frameworks::core_graphics::cg_bitmap_context::{
    CGBitmapContextCreate, CGBitmapContextCreateImage,
};
use crate::frameworks::core_graphics::cg_color_space::CGColorSpaceCreateDeviceRGB;
use crate::frameworks::core_graphics::cg_context::{
    CGContextFillRect, CGContextRelease, CGContextScaleCTM, CGContextSetRGBFillColor,
    CGContextTranslateCTM,
};
use crate::frameworks::core_graphics::cg_image::{self, kCGImageAlphaPremultipliedLast};
use crate::frameworks::core_graphics::{CGFloat, CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::ns_run_loop::run_run_loop_single_iteration;
use crate::frameworks::foundation::ns_string;
use crate::frameworks::uikit::ui_font::{
    UITextAlignmentCenter, UITextAlignmentLeft, UITextAlignmentRight,
};
use crate::frameworks::uikit::ui_graphics::{UIGraphicsPopContext, UIGraphicsPushContext};
use crate::frameworks::uikit::ui_view::ui_control::ui_button::{
    UIButtonTypeCustom,
};
use crate::frameworks::uikit::ui_view::ui_control::{
    UIControlEventTouchUpInside, UIControlStateNormal,
};
use crate::fs::BundleData;
use crate::image::Image;
use crate::mem::Ptr;
use crate::objc::{id, msg, msg_class, nil, objc_classes, release, ClassExports, HostObject};
use crate::options::Options;
use crate::paths;
use crate::Environment;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

struct AppInfo {
    path: PathBuf,
    display_name: String,
    icon: Option<Image>,
    /// `NSString*`
    display_name_ns_string: Option<id>,
    /// `UIImage*`
    icon_ui_image: Option<id>,
}

pub fn app_picker(options: Options) -> Result<(PathBuf, Vec<String>), String> {
    let apps_dir = paths::user_data_base_path().join(paths::APPS_DIR);

    let apps: Result<Vec<AppInfo>, String> = if !apps_dir.is_dir() {
        Err(format!("The {} directory couldn't be found. Check you're running touchHLE from the right directory.", apps_dir.display()))
    } else {
        enumerate_apps(&apps_dir)
            .map_err(|err| {
                format!(
                    "Couldn't get list of apps in the {} directory: {}.",
                    apps_dir.display(),
                    err
                )
            })
            .and_then(|apps| {
                if apps.is_empty() {
                    Err(format!(
                        "No apps were found in the {} directory.",
                        apps_dir.display()
                    ))
                } else {
                    Ok(apps)
                }
            })
    };

    show_app_picker_gui(options, apps)
}

fn enumerate_apps(apps_dir: &Path) -> Result<Vec<AppInfo>, std::io::Error> {
    let mut apps = Vec::new();
    for app in std::fs::read_dir(apps_dir)? {
        let app_path = app?.path();
        if app_path.extension() != Some(OsStr::new("app"))
            && app_path.extension() != Some(OsStr::new("ipa"))
        {
            continue;
        }

        let (bundle, fs) = match BundleData::open_any(&app_path).and_then(|bundle_data| {
            Bundle::new_bundle_and_fs_from_host_path(bundle_data, /* read_only_mode: */ true)
        }) {
            Ok(ok) => ok,
            Err(e) => {
                log!(
                    "Warning: couldn't open app bundle {}: {} (skipping)",
                    app_path.display(),
                    e
                );
                continue;
            }
        };

        let display_name = bundle.display_name().to_owned();

        let icon = match bundle.load_icon(&fs) {
            Ok(icon) => Some(icon),
            Err(e) => {
                log!("Warning: couldn't load icon for app bundle {}: {} (displaying placeholder instead)", app_path.display(), e);
                None
            }
        };

        apps.push(AppInfo {
            path: app_path,
            display_name,
            icon,
            display_name_ns_string: None,
            icon_ui_image: None,
        });
    }

    apps.sort_by_key(|app| app.display_name.to_uppercase());

    Ok(apps)
}

#[derive(Default)]
struct AppPickerDelegateHostObject {
    icon_tapped: id,
}
impl HostObject for AppPickerDelegateHostObject {}

pub const DYLIB: crate::dyld::HostDylib = crate::dyld::HostDylib {
    path: "/.touchHLE/AppPickerHelpers.dylib",
    aliases: &[],
    class_exports: &[CLASSES],
    constant_exports: &[],
    function_exports: &[],
};

const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation _touchHLE_AppPickerDelegate: NSObject

- (())iconTapped:(id)sender {
    let host_obj = env.objc.borrow_mut::<AppPickerDelegateHostObject>(this);
    host_obj.icon_tapped = sender;
}

@end

};

fn show_app_picker_gui(
    options: Options,
    apps: Result<Vec<AppInfo>, String>,
) -> Result<(PathBuf, Vec<String>), String> {
    let icon = {
        let bytes: &[u8] = match crate::branding() {
            "" => include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/res/icon.png")),
            "UNOFFICIAL" => include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/res/icon_unofficial.png"
            )),
            "PREVIEW" => {
                include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/res/icon_preview.png"))
            }
            _ => panic!(),
        };
        let mut image = Image::from_bytes(bytes).unwrap();
        image.round_corners(
            (10.0 / 57.0) * (image.dimensions().0 as f32),
            /* four_corners: */ true,
            /* add_sheen: */ true,
        );
        image
    };
    let environment = Environment::new_without_app(options, icon)?;
    Ok(environment.run_app_picker(|env| app_picker_inner(env, apps)))
}

fn app_picker_inner(
    env: &mut Environment,
    mut apps: Result<Vec<AppInfo>, String>,
) -> (PathBuf, Vec<String>) {
    let option_args = Vec::new();

    let ui_application: id = msg_class![env; UIApplication new];
    let delegate = env
        .objc
        .get_known_class("_touchHLE_AppPickerDelegate", &mut env.mem);
    let delegate = env.objc.alloc_object(
        delegate,
        Box::<AppPickerDelegateHostObject>::default(),
        &mut env.mem,
    );
    () = msg![env; ui_application setDelegate:delegate];

    let screen: id = msg_class![env; UIScreen mainScreen];
    let bounds: CGRect = msg![env; screen bounds];

    let window: id = msg_class![env; UIWindow alloc];
    let window: id = msg![env; window initWithFrame:bounds];

    let app_frame: CGRect = msg![env; screen applicationFrame];
    let main_view: id = msg_class![env; UIView alloc];
    let main_view: id = msg![env; main_view initWithFrame:app_frame];
    () = msg![env; window addSubview:main_view];

    // Wallpaper
    let mut found_wallpaper = false;
    let mut have_wallpaper = false;
    for candidate in paths::WALLPAPER_FILES {
        let candidate = paths::user_data_base_path().join(candidate);
        if !candidate.exists() {
            continue;
        }
        found_wallpaper = true;

        let image = match std::fs::read(&candidate) {
            Ok(image) => image,
            Err(e) => {
                log!("Warning: couldn't read {}: {}", candidate.display(), e);
                break;
            }
        };
        let image = match Image::from_bytes(&image) {
            Ok(image) => image,
            Err(e) => {
                log!("Warning: couldn't decode {}: {}", candidate.display(), e);
                break;
            }
        };

        let image = cg_image::from_image(env, image);
        let image: id = msg_class![env; UIImage imageWithCGImage:image];
        let wallpaper: id = msg_class![env; UIImageView alloc];
        let wallpaper: id = msg![env; wallpaper initWithImage:image];
        () = msg![env; wallpaper setFrame:(CGRect {
            origin: CGPoint {
                x: 0.0,
                y: 0.0,
            },
            size: app_frame.size,
        })];
        () = msg![env; wallpaper setAlpha:(0.5 as CGFloat)];
        () = msg![env; main_view addSubview:wallpaper];
        have_wallpaper = true;
        break;
    }
    if !found_wallpaper {
        let CGSize { width, height } = app_frame.size;
        log!(
            "No wallpaper found; filename can be one of: {}; ideal size is {}×{} pixels",
            paths::WALLPAPER_FILES.join(", "),
            width,
            height,
        );
    }

    // Version label
    {
        let label_frame = CGRect {
            origin: CGPoint {
                x: 0.0,
                y: app_frame.size.height - 20.0,
            },
            size: CGSize {
                width: app_frame.size.width - 5.0,
                height: 15.0,
            },
        };
        let label: id = msg_class![env; UILabel alloc];
        let label: id = msg![env; label initWithFrame:label_frame];
        let text = ns_string::from_rust_string(
            env,
            format!(
                "iTOUCH {}{}{}",
                crate::branding(),
                if crate::branding().is_empty() {
                    ""
                } else {
                    " "
                },
                crate::VERSION
            ),
        );
        () = msg![env; label setText:text];
        () = msg![env; label setTextAlignment:UITextAlignmentRight];
        let font_size: CGFloat = 12.0;
        let font: id = msg_class![env; UIFont systemFontOfSize:font_size];
        () = msg![env; label setFont:font];
        let text_color: id = if have_wallpaper {
            msg_class![env; UIColor whiteColor]
        } else {
            msg_class![env; UIColor lightGrayColor]
        };
        () = msg![env; label setTextColor:text_color];
        let bg_color: id = msg_class![env; UIColor clearColor];
        () = msg![env; label setBackgroundColor:bg_color];
        () = msg![env; main_view addSubview:label];
    }

    let brand_color: id = if crate::branding() == "UNOFFICIAL" {
        msg_class![env; UIColor redColor]
    } else {
        msg_class![env; UIColor grayColor]
    };

    for i in 1..=7 {
        let label_frame = CGRect {
            origin: CGPoint {
                x: 0.0,
                y: (app_frame.size.height / 8.0) * (i as f32) - 25.0,
            },
            size: CGSize {
                width: app_frame.size.width,
                height: 50.0,
            },
        };
        let label: id = msg_class![env; UILabel alloc];
        let label: id = msg![env; label initWithFrame:label_frame];
        let text = ns_string::from_rust_string(env, crate::branding().to_owned());
        () = msg![env; label setText:text];
        () = msg![env; label setTextAlignment:(if i % 2 == 0 { UITextAlignmentLeft } else { UITextAlignmentRight })];
        let font_size: CGFloat = 48.0;
        let font: id = msg_class![env; UIFont systemFontOfSize:font_size];
        () = msg![env; label setFont:font];
        () = msg![env; label setTextColor:brand_color];
        let bg_color: id = msg_class![env; UIColor clearColor];
        () = msg![env; label setBackgroundColor:bg_color];
        () = msg![env; main_view addSubview:label];
    }

    let divider = app_frame.size.height - 100.0;

    let mut icon_grid_stuff = match &mut apps {
        Ok(ref mut apps) => {
            let mut icon_grid_stuff = make_icon_grid(
                env,
                delegate,
                main_view,
                app_frame,
                apps.len(),
                have_wallpaper,
            );
            update_icon_grid(env, &mut icon_grid_stuff, apps, 0);
            Some(icon_grid_stuff)
        }
        Err(e) => {
            let label_frame = CGRect {
                origin: CGPoint { x: 10.0, y: 10.0 },
                size: CGSize {
                    width: app_frame.size.width - 20.0,
                    height: divider - 20.0,
                },
            };
            let label: id = msg_class![env; UILabel alloc];
            let label: id = msg![env; label initWithFrame:label_frame];
            let text = ns_string::from_rust_string(env, e.clone());
            () = msg![env; label setText:text];
            () = msg![env; label setTextAlignment:UITextAlignmentCenter];
            () = msg![env; label setNumberOfLines:0]; // unlimited
            let text_color: id = msg_class![env; UIColor lightGrayColor];
            () = msg![env; label setTextColor:text_color];
            let bg_color: id = msg_class![env; UIColor clearColor];
            () = msg![env; label setBackgroundColor:bg_color];
            () = msg![env; main_view addSubview:label];
            None
        }
    };

    () = msg![env; window makeKeyAndVisible];

    let main_run_loop: id = msg_class![env; NSRunLoop mainRunLoop];
    let app_path = loop {
        run_run_loop_single_iteration(env, main_run_loop);
        let host_obj = env.objc.borrow_mut::<AppPickerDelegateHostObject>(delegate);
        let icon_tapped = std::mem::take(&mut host_obj.icon_tapped);
        if icon_tapped != nil {
            match icon_grid_stuff.as_ref().unwrap().icon_map.get(&icon_tapped) {
                Some(&TappedIcon::App(app_idx)) => {
                    () = msg![env; icon_tapped setAlpha:(0.5 as CGFloat)];
                    crate::frameworks::core_animation::recomposite_if_necessary(
                        env, /* force: */ true,
                    );
                    run_run_loop_single_iteration(env, main_run_loop);

                    let app_path = &apps.as_ref().unwrap()[app_idx].path;
                    echo!("Picked: {}", app_path.display());
                    break app_path.clone();
                }
                Some(&TappedIcon::ChangePage(page_idx)) => {
                    update_icon_grid(
                        env,
                        icon_grid_stuff.as_mut().unwrap(),
                        apps.as_mut().unwrap(),
                        page_idx,
                    );
                }
                None => (),
            }
            continue;
        }
    };

    (app_path, option_args)
}

const ICON_SIZE: CGSize = CGSize {
    width: 57.0,
    height: 57.0,
};

enum TappedIcon {
    App(usize),
    ChangePage(usize),
}

struct IconGridStuff {
    icon_buttons_and_labels: Vec<(id, id)>,
    placeholder_icon: Option<id>,
    prev_icon: Option<id>,
    next_icon: Option<id>,
    pages: Vec<std::ops::Range<usize>>,
    icon_map: HashMap<id, TappedIcon>,
}

fn make_icon_grid(
    env: &mut Environment,
    delegate: id,
    main_view: id,
    app_frame: CGRect,
    total_app_count: usize,
    have_wallpaper: bool,
) -> IconGridStuff {
    let num_cols = 4;
    let num_cols_f = num_cols as CGFloat;
    let num_rows = 4;
    let label_size = CGSize {
        width: 74.0,
        height: 13.0,
    };
    let icon_gap_x: CGFloat = 19.0;
    let icon_gap_y: CGFloat = 4.0 + label_size.height + 14.0;
    let icon_grid_width = (ICON_SIZE.width * num_cols_f) + icon_gap_x * (num_cols_f - 1.0);
    let icon_grid_origin = CGPoint {
        x: (app_frame.size.width - icon_grid_width) / 2.0,
        y: 12.0,
    };

    let icon_tapped_sel = env.objc.lookup_selector("iconTapped:").unwrap();

    let mut icon_buttons_and_labels = Vec::new();

    for i in 0..(num_cols * num_rows) {
        let col = i % num_cols;
        let row = i / num_cols;

        let icon_frame = CGRect {
            origin: CGPoint {
                x: (icon_grid_origin.x + (col as CGFloat) * (ICON_SIZE.width + icon_gap_x)).round(),
                y: (icon_grid_origin.y + (row as CGFloat) * (ICON_SIZE.height + icon_gap_y))
                    .round(),
            },
            size: ICON_SIZE,
        };
        let icon_button: id = msg_class![env; UIButton buttonWithType:UIButtonTypeCustom];
        () = msg![env; icon_button setFrame:icon_frame];
        let image_view: id = msg![env; icon_button imageView];
        let bounds: CGRect = msg![env; icon_button bounds];
        () = msg![env; image_view setFrame:bounds];
        () = msg![env; icon_button addTarget:delegate
                                      action:icon_tapped_sel
                            forControlEvents:UIControlEventTouchUpInside];
        () = msg![env; main_view addSubview:icon_button];

        let label_frame = CGRect {
            origin: CGPoint {
                x: (icon_frame.origin.x - (label_size.width - ICON_SIZE.width) / 2.0).round(),
                y: (icon_frame.origin.y + ICON_SIZE.height + 4.0).round(),
            },
            size: label_size,
        };
        let label: id = msg_class![env; UILabel alloc];
        let label: id = msg![env; label initWithFrame:label_frame];
        () = msg![env; label setTextAlignment:UITextAlignmentCenter];
        let font_size: CGFloat = label_size.height - 2.0;
        let font: id = if have_wallpaper {
            msg_class![env; UIFont systemFontOfSize:font_size]
        } else {
            msg_class![env; UIFont boldSystemFontOfSize:font_size]
        };
        () = msg![env; label setFont:font];
        let text_color: id = if have_wallpaper {
            msg_class![env; UIColor whiteColor]
        } else {
            msg_class![env; UIColor lightGrayColor]
        };
        () = msg![env; label setTextColor:text_color];
        let bg_color: id = msg_class![env; UIColor clearColor];
        () = msg![env; label setBackgroundColor:bg_color];
        () = msg![env; main_view addSubview:label];

        icon_buttons_and_labels.push((icon_button, label));
    }

    let mut pages = Vec::new();
    let mut start = 0;
    while start < total_app_count {
        let mut end = start + icon_buttons_and_labels.len();
        if start > 0 {
            end -= 1;
        }
        if end < total_app_count {
            end -= 1;
        } else {
            end = total_app_count;
        }
        pages.push(start..end);
        start = end;
    }

    IconGridStuff {
        icon_buttons_and_labels,
        placeholder_icon: None,
        prev_icon: None,
        next_icon: None,
        pages,
        icon_map: HashMap::new(),
    }
}

fn make_icon_from_glyph(
    env: &mut Environment,
    glyph: char,
    font_size: CGFloat,
    baseline_offset: CGFloat,
    bg_color: (CGFloat, CGFloat, CGFloat, CGFloat),
) -> id {
    let color_space = CGColorSpaceCreateDeviceRGB(env);
    let context = CGBitmapContextCreate(
        env,
        Ptr::null(),
        ICON_SIZE.width as u32,
        ICON_SIZE.height as u32,
        8,
        4 * (ICON_SIZE.width as u32),
        color_space,
        kCGImageAlphaPremultipliedLast,
    );
    UIGraphicsPushContext(env, context);

    CGContextTranslateCTM(env, context, 0.0, ICON_SIZE.height);
    CGContextScaleCTM(env, context, 1.0, -1.0);

    let (r, g, b, a) = bg_color;
    CGContextSetRGBFillColor(env, context, r, g, b, a);
    CGContextFillRect(
        env,
        context,
        CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: ICON_SIZE,
        },
    );

    let font: id = msg_class![env; UIFont systemFontOfSize:font_size];
    let glyph_string: id = ns_string::from_rust_string(env, [glyph].into_iter().collect());
    let glyph_size: CGSize = msg![env; glyph_string sizeWithFont:font];
    CGContextSetRGBFillColor(env, context, 1.0, 1.0, 1.0, 1.0);
    let glyph_origin = CGPoint {
        x: ICON_SIZE.width / 2.0 - glyph_size.width / 2.0,
        y: ICON_SIZE.height / 2.0 - glyph_size.height / 2.0 + baseline_offset,
    };
    let _: CGSize = msg![env; glyph_string drawAtPoint:glyph_origin withFont:font];
    release(env, glyph_string);

    UIGraphicsPopContext(env);

    let cg_image = CGBitmapContextCreateImage(env, context);
    cg_image::borrow_image_mut(&mut env.objc, cg_image).round_corners(
        (10.0 / 57.0) * ICON_SIZE.width,
        /* four_corners: */ true,
        /* add_sheen: */ true,
    );
    CGContextRelease(env, context);

    let ui_image: id = msg_class![env; UIImage imageWithCGImage:cg_image];
    release(env, cg_image);

    ui_image
}

fn update_icon_grid(
    env: &mut Environment,
    icon_grid_stuff: &mut IconGridStuff,
    apps: &mut [AppInfo],
    page_idx: usize,
) {
    icon_grid_stuff.icon_map.clear();

    let app_idx_range = icon_grid_stuff.pages[page_idx].clone();
    let have_prev_icon = page_idx != 0;
    let have_next_icon = app_idx_range.end != apps.len();

    let mut icon_iter = icon_grid_stuff.icon_buttons_and_labels.iter();

    if have_prev_icon {
        let &(icon_button, label) = icon_iter.next().unwrap();
        let image = *icon_grid_stuff.prev_icon.get_or_insert_with(|| {
            make_icon_from_glyph(env, '←', 50.0, -9.0, (0.25, 0.25, 0.25, 1.0))
        });
        () = msg![env; icon_button setImage:image forState:UIControlStateNormal];
        () = msg![env; label setText:(ns_string::get_static_str(env, ""))];
        icon_grid_stuff
            .icon_map
            .insert(icon_button, TappedIcon::ChangePage(page_idx - 1));
    }

    for app_idx in app_idx_range.clone() {
        let app = &mut apps[app_idx];

        let &(icon_button, label) = icon_iter.next().unwrap();

        if let Some(icon) = app.icon.take() {
            let image = cg_image::from_image(env, icon);
            let image: id = msg_class![env; UIImage imageWithCGImage:image];
            app.icon_ui_image = Some(image);
        }

        let image = app.icon_ui_image.unwrap_or_else(|| {
            *icon_grid_stuff.placeholder_icon.get_or_insert_with(|| {
                make_icon_from_glyph(env, '?', 40.0, 0.0, (0.5, 0.5, 0.5, 1.0))
            })
        });
        () = msg![env; icon_button setImage:image forState:UIControlStateNormal];

        let text = *app
            .display_name_ns_string
            .get_or_insert_with(|| ns_string::from_rust_string(env, app.display_name.clone()));
        () = msg![env; label setText:text];

        icon_grid_stuff
            .icon_map
            .insert(icon_button, TappedIcon::App(app_idx));
    }

    if have_next_icon {
        let &(icon_button, label) = icon_iter.next().unwrap();
        let image = *icon_grid_stuff.next_icon.get_or_insert_with(|| {
            make_icon_from_glyph(env, '→', 50.0, -9.0, (0.25, 0.25, 0.25, 1.0))
        });
        () = msg![env; icon_button setImage:image forState:UIControlStateNormal];
        () = msg![env; label setText:(ns_string::get_static_str(env, ""))];
        icon_grid_stuff
            .icon_map
            .insert(icon_button, TappedIcon::ChangePage(page_idx + 1));
    }

    for &(icon_button, label) in icon_iter {
        () = msg![env; icon_button setImage:nil forState:UIControlStateNormal];
        () = msg![env; label setText:(ns_string::get_static_str(env, ""))];
    }
}
