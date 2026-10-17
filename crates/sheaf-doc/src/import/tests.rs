use super::*;
use lopdf::{Document as Pdf, Stream, dictionary};
use sheaf_core::Rotation;

fn mark(kind: AnnotKind) -> Annot {
    Annot {
        id: AnnotId(7),
        kind,
        color: [220, 40, 40],
        opacity: 0.8,
        stroke: 2.0,
        author: "Ann".into(),
        contents: "Look here".into(),
        modified: "D:20261001120000".into(),
        created: "D:20260930120000".into(),
        fill: None,
        dashed: false,
        cloudy: false,
        turn: Rotation::R0,
        name: String::new(),
    }
}

/// 1 in = 10 ft, as a calibration would leave it.
fn feet() -> crate::measure::Scale {
    crate::measure::Scale {
        page: 1.0,
        page_unit: "in".into(),
        real: 10.0,
        unit: "ft".into(),
        precision: 2,
    }
}

fn b(x: f32, y: f32, w: f32, h: f32) -> Box2 {
    Box2 { x, y, w, h }
}

fn p(x: f32, y: f32) -> Pt {
    Pt { x, y }
}

/// Whether two values are the same to within 0.05 in every number.
fn close(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    use serde_json::Value::*;
    match (a, b) {
        (Number(x), Number(y)) => (x.as_f64().unwrap() - y.as_f64().unwrap()).abs() < 0.05,
        (Array(x), Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(a, b)| close(a, b)),
        (Object(x), Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| close(v, w)))
        }
        _ => a == b,
    }
}

/// `a`, turned `turn`, as sheaf writes it, read back as another tool's on
/// a page that shows it so: the same mark.
fn round_trip(a: &Annot, turn: Rotation) {
    let a = &Annot { turn, ..a.clone() };
    let mut pdf = Pdf::with_version("1.7");
    let mut main = crate::annot::to_pdf(a, &mut pdf).main;
    main.remove(b"SheafAnnot");
    let back = convert(&pdf, &main, turn, Options::default())
        .unwrap_or_else(|e| panic!("{:?}: {e:?}", a.kind));
    let rotate = turn.degrees();
    let (want, got) = (
        serde_json::to_value(&a.kind).unwrap(),
        serde_json::to_value(&back.kind).unwrap(),
    );
    assert!(close(&want, &got), "at {rotate}°\nwant {want}\n got {got}");
    assert_eq!(back.name, a.name());
    assert_eq!(
        (&back.author[..], &back.modified[..], &back.created[..]),
        (&a.author[..], &a.modified[..], &a.created[..])
    );
    assert_eq!(back.turn, turn);
    assert!((back.opacity - a.opacity).abs() < 0.01);
    assert_eq!(
        (back.fill, back.dashed, back.cloudy),
        (a.fill, a.dashed, a.cloudy)
    );
    if !matches!(
        a.kind,
        AnnotKind::FreeText { .. } | AnnotKind::Callout { .. }
    ) {
        assert_eq!(back.color, a.color, "{:?}", a.kind);
        assert!(
            (back.stroke - a.stroke).abs() < 0.01
                || matches!(
                    a.kind,
                    AnnotKind::Note { .. }
                        | AnnotKind::Highlight { .. }
                        | AnnotKind::Underline { .. }
                        | AnnotKind::StrikeOut { .. }
                        | AnnotKind::Squiggly { .. }
                        | AnnotKind::Insert { .. }
                )
        );
    }
}

#[test]
fn our_own_marks_read_back_as_themselves() {
    let rects = vec![b(100.0, 100.0, 120.0, 12.0), b(100.0, 114.0, 60.0, 12.0)];
    let mut square = mark(AnnotKind::Square {
        rect: b(50.0, 60.0, 80.0, 40.0),
    });
    square.fill = Some([255, 255, 0]);
    square.dashed = true;
    let mut cloud = mark(AnnotKind::Polygon {
        points: vec![p(300.0, 300.0), p(400.0, 310.0), p(350.0, 400.0)],
    });
    cloud.cloudy = true;
    let marks = vec![
        mark(AnnotKind::Highlight {
            rects: rects.clone(),
        }),
        mark(AnnotKind::Underline {
            rects: rects.clone(),
        }),
        mark(AnnotKind::StrikeOut {
            rects: rects.clone(),
        }),
        mark(AnnotKind::Squiggly { rects }),
        mark(AnnotKind::Note {
            pos: p(100.0, 200.0),
        }),
        mark(AnnotKind::FreeText {
            rect: b(100.0, 300.0, 150.0, 40.0),
            size: 11.0,
        }),
        square,
        mark(AnnotKind::Ellipse {
            rect: b(200.0, 60.0, 50.0, 30.0),
        }),
        mark(AnnotKind::Line {
            a: p(10.0, 10.0),
            b: p(200.0, 150.0),
            arrow: true,
            arrow_start: false,
        }),
        mark(AnnotKind::Ink {
            paths: vec![vec![p(10.0, 10.0), p(20.0, 25.0), p(40.0, 30.0)]],
        }),
        cloud,
        mark(AnnotKind::Polyline {
            points: vec![p(30.0, 500.0), p(90.0, 520.0), p(150.0, 480.0)],
        }),
        mark(AnnotKind::Callout {
            rect: b(300.0, 500.0, 150.0, 36.0),
            knee: p(280.0, 518.0),
            point: p(220.0, 580.0),
            size: 12.0,
        }),
        mark(AnnotKind::Insert {
            at: b(150.0, 150.0, 6.0, 9.0),
        }),
        mark(AnnotKind::Measure {
            what: crate::measure::Measured::Distance,
            points: vec![p(100.0, 400.0), p(300.0, 450.0)],
            scale: Box::new(feet()),
        }),
        mark(AnnotKind::Measure {
            what: crate::measure::Measured::Perimeter,
            points: vec![p(100.0, 500.0), p(200.0, 520.0), p(260.0, 600.0)],
            scale: Box::new(feet()),
        }),
        mark(AnnotKind::Measure {
            what: crate::measure::Measured::Area,
            points: vec![p(300.0, 600.0), p(400.0, 600.0), p(400.0, 700.0)],
            scale: Box::new(feet()),
        }),
    ];
    for turn in [Rotation::R0, Rotation::R90] {
        for a in &marks {
            round_trip(a, turn);
        }
    }
}

#[test]
fn free_text_takes_its_color_from_its_appearance_string() {
    let pdf = Pdf::with_version("1.7");
    let dict = dictionary! {
        "Subtype" => "FreeText",
        "Rect" => vec![100.into(), 600.into(), 300.into(), 640.into()],
        "DA" => Object::string_literal("/Helv 9 Tf 0 0 1 rg"),
        "C" => vec![1.into(), 1.into(), 0.into()],
        "Contents" => Object::string_literal("Tick and tie"),
    };
    let a = convert(&pdf, &dict, Rotation::R0, Options::default()).unwrap();
    assert_eq!(a.color, [0, 0, 255]);
    let AnnotKind::FreeText { rect, size } = a.kind else {
        panic!("{:?}", a.kind)
    };
    assert_eq!(size, 9.0);
    // No /RD: in by half the border, which defaults to 1.
    assert!(close(
        &serde_json::to_value(rect).unwrap(),
        &serde_json::to_value(b(100.5, 600.5, 199.0, 39.0)).unwrap()
    ));
    // Without /NM, it's named as the panel names it.
    assert_eq!(a.name, thread_name(&dict));
    assert!(a.name.starts_with("sheaf-anon:"));
}

#[test]
fn replies_statuses_and_replacements_keep_their_threads() {
    let mut pdf = Pdf::with_version("1.7");
    let r = || vec![100.into(), 100.into(), 110.into(), 112.into()];
    let caret = pdf.add_object(dictionary! {
        "Subtype" => "Caret", "Rect" => r(), "NM" => Object::string_literal("c1"),
        "Contents" => Object::string_literal("the"),
    });
    let strike = dictionary! {
        "Subtype" => "StrikeOut", "Rect" => r(), "IRT" => caret, "RT" => "Group",
        "QuadPoints" => vec![110.into(), 112.into(), 140.into(), 112.into(), 110.into(), 100.into(), 140.into(), 100.into()],
    };
    let reply = dictionary! {
        "Subtype" => "Text", "Rect" => r(), "IRT" => caret,
        "Contents" => Object::string_literal("Agreed"),
    };
    let status = dictionary! {
        "Subtype" => "Text", "Rect" => r(), "IRT" => caret,
        "State" => Object::string_literal("Accepted"),
    };
    // As XFDF gives it: the name itself.
    let by_name = dictionary! {
        "Subtype" => "Text", "Rect" => r(), "IRT" => Object::string_literal("c1"),
        "StateModel" => Object::string_literal("Marked"), "State" => Object::string_literal("Marked"),
    };
    let popup = dictionary! { "Subtype" => "Popup", "Rect" => r() };
    let sound = dictionary! { "Subtype" => "Sound", "Rect" => r() };
    let caret_dict = pdf.get_dictionary(caret).unwrap().clone();
    let all = [
        &caret_dict,
        &strike,
        &reply,
        &status,
        &by_name,
        &popup,
        &sound,
    ];
    let out = convert_page(&pdf, &all, Rotation::R0, Options::default());
    match &out[0] {
        Ok(Annot {
            kind: AnnotKind::Replace { rects, at, .. },
            name,
            contents,
            ..
        }) => {
            assert_eq!(name, "c1");
            assert_eq!(contents, "the");
            assert!(close(
                &serde_json::to_value(rects).unwrap(),
                &serde_json::to_value([b(110.0, 100.0, 30.0, 12.0)]).unwrap()
            ));
            assert!(close(
                &serde_json::to_value(at).unwrap(),
                &serde_json::to_value(b(100.0, 100.0, 10.0, 12.0)).unwrap()
            ));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(out[1], Err(NotAMark::Part));
    assert!(
        matches!(&out[2], Ok(Annot { kind: AnnotKind::Reply { parent }, .. }) if parent == "c1")
    );
    assert!(
        matches!(&out[3], Ok(Annot { kind: AnnotKind::Status { parent, model, state }, .. })
        if parent == "c1" && model == REVIEW && state == "Accepted")
    );
    assert!(
        matches!(&out[4], Ok(Annot { kind: AnnotKind::Status { model, .. }, .. }) if model == "Marked")
    );
    assert_eq!(out[5], Err(NotAMark::NotComment));
    assert_eq!(out[6], Err(NotAMark::Unsupported("Sound".into())));
}

#[test]
fn a_stamp_keeps_its_appearance() {
    let mut pdf = Pdf::with_version("1.7");
    let form = pdf.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject", "Subtype" => "Form",
            "BBox" => vec![0.into(), 0.into(), 100.into(), 40.into()],
        },
        format!("0 0 1 rg 0 0 100 40 re f % {}", fastrand::u64(..)).into_bytes(),
    ));
    let dict = dictionary! {
        "Subtype" => "Stamp", "Name" => "SBApproved",
        "Rect" => vec![100.into(), 600.into(), 200.into(), 640.into()],
        "AP" => dictionary! { "N" => form },
    };
    let a = convert(&pdf, &dict, Rotation::R0, Options::default()).unwrap();
    let AnnotKind::PageStamp {
        stamp, name, rect, ..
    } = &a.kind
    else {
        panic!("{:?}", a.kind)
    };
    assert_eq!(name, "Approved");
    assert_eq!(*rect, b(100.0, 600.0, 100.0, 40.0));
    let page = Pdf::load(crate::generated::path_of(stamp).unwrap()).unwrap();
    assert_eq!(page.get_pages().len(), 1);
    // With no appearance, its name in a frame.
    let mut bare = dict.clone();
    bare.remove(b"AP");
    let a = convert(&pdf, &bare, Rotation::R0, Options::default()).unwrap();
    assert!(matches!(&a.kind, AnnotKind::TextStamp { text, .. } if text == "APPROVED"));
}
