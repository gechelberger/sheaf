//! The content writer: what each call writes, names bound at the end, and
//! the same bytes from the same calls on any thread.

use lopdf::{Dictionary, Document as Pdf, Object, dictionary};

use super::*;
use crate::annot::{Box2, Pt};
use crate::geom::{Affine, Rect};

/// The text of what `w` wrote, with no resources reserved or given.
fn written(w: ContentWriter) -> (String, ResourceNeeds) {
    let (bytes, needs) = w.finish(&Reserved::default(), &[]);
    (String::from_utf8(bytes).unwrap(), needs)
}

fn list(subtype: &str) -> Dictionary {
    dictionary! { "Type" => "Pagination", "Subtype" => Object::Name(subtype.into()) }
}

/// A name `Sh` and six base32 characters, counted up by one.
fn next(name: &[u8]) -> Vec<u8> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let n = name[2..].iter().fold(0u32, |n, c| {
        n * 32 + ALPHABET.iter().position(|a| a == c).unwrap() as u32
    });
    let n = (n + 1) & ((1 << 30) - 1);
    let mut out = b"Sh".to_vec();
    out.extend((0..6).rev().map(|i| ALPHABET[(n >> (5 * i)) as usize & 31]));
    out
}

#[test]
fn each_call_writes_its_operator() {
    let mut w = ContentWriter::new();
    w.save();
    w.transform(Affine {
        a: 2.0,
        e: -0.00001,
        f: 1.0 / 3.0,
        ..Affine::IDENTITY
    });
    w.fill_color(Color::Gray(0.5));
    w.stroke_color(Color::Rgb([1.0, 0.0, 0.25]));
    w.fill_color(Color::Cmyk([0.0, 0.1, 0.2, 1.0]));
    w.stroke_color(Color::rgb8([255, 0, 51]));
    w.line_width(0.75);
    w.dash(&[4.0, 3.0], 0.0);
    w.dash(&[], 0.0);
    w.line_cap(LineCap::Round);
    w.line_join(LineJoin::Bevel);
    w.move_to(1.0, 2.0);
    w.line_to(612.0, 1e7);
    w.curve_to(
        Pt { x: 1.0, y: 2.0 },
        Pt { x: 3.0, y: 4.0 },
        Pt {
            x: 5.0,
            y: f32::NAN,
        },
    );
    w.rect(0.0, 0.0, 10.0, -0.0);
    w.close();
    for paint in [
        Paint::Fill,
        Paint::FillEvenOdd,
        Paint::Stroke,
        Paint::CloseStroke,
        Paint::FillStroke,
        Paint::FillStrokeEvenOdd,
        Paint::CloseFillStroke,
        Paint::CloseFillStrokeEvenOdd,
    ] {
        w.paint(paint);
    }
    w.clip(FillRule::NonZero);
    w.clip(FillRule::EvenOdd);
    w.paint(Paint::None);
    w.begin_text();
    w.text_mode(TextMode::Invisible);
    w.horizontal_scale(160.0);
    w.leading(12.5);
    w.text_move(72.0, 700.0);
    w.text_matrix(Affine::IDENTITY);
    w.next_line();
    w.end_text();
    w.restore();
    w.end_marked();
    let (text, needs) = written(w);
    assert_eq!(
        text,
        "q\n2 0 0 1 0 0.3333 cm\n0.5 g\n1 0 0.25 RG\n0 0.1 0.2 1 k\n1 0 0.2 RG\n\
         0.75 w\n[4 3] 0 d\n[] 0 d\n1 J\n2 j\n1 2 m\n612 10000000 l\n1 2 3 4 5 0 c\n\
         0 0 10 0 re\nh\nf\nf*\nS\ns\nB\nB*\nb\nb*\nW\nW*\nn\n\
         BT\n3 Tr\n160 Tz\n12.5 TL\n72 700 Td\n1 0 0 1 0 0 Tm\nT*\nET\nQ\nEMC\n"
    );
    assert_eq!(needs, ResourceNeeds::default());
}

/// Text in a font, the font named; a string with a control code is
/// written in hex, others literal and escaped.
#[test]
fn text_is_written_in_its_font() {
    let mut w = ContentWriter::new();
    assert_eq!(w.text("lost"), Unencoded(4), "no font yet");
    w.font(&Std14::Helvetica, 9.0);
    assert_eq!(w.text("(a\\b) é"), Unencoded(0));
    assert_eq!(w.text("中文"), Unencoded(2));
    w.save();
    w.font(&Glyphless, 10.0);
    w.text("Hi");
    w.restore();
    // The font `q` saved is back.
    w.text("é");
    let (bytes, needs) = w.finish(&Reserved::default(), &[]);
    let [helvetica, glyphless] = &needs.0[..] else {
        panic!("{needs:?}")
    };
    let mut want = format!("/{} 9 Tf\n", String::from_utf8_lossy(&helvetica.name)).into_bytes();
    want.extend_from_slice(b"(\\(a\\\\b\\) \xe9) Tj\n(??) Tj\nq\n");
    want.extend(format!("/{} 10 Tf\n", String::from_utf8_lossy(&glyphless.name)).bytes());
    want.extend_from_slice(b"<00480069> Tj\nQ\n(\xe9) Tj\n");
    assert_eq!(bytes, want, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(helvetica.kind, ResKind::Font);
    assert_eq!(helvetica.res, Std14::Helvetica.resource().into());
    assert_eq!(glyphless.res, Glyphless.resource().into());
}

/// Property lists are named, never inline; an ExtGState is a resource.
#[test]
fn marked_content_names_its_list() {
    let mut w = ContentWriter::new();
    w.begin_marked("Artifact", list("Header"));
    w.gstate(GState {
        fill_alpha: Some(0.5),
        stroke_alpha: Some(2.0),
        blend: Some(Blend::Multiply),
    });
    w.end_marked();
    w.begin_marked("A b/c", list("Header"));
    w.end_marked();
    let (text, needs) = written(w);
    let [props, gs] = &needs.0[..] else {
        panic!("{needs:?}")
    };
    let (p, g) = (
        String::from_utf8_lossy(&props.name),
        String::from_utf8_lossy(&gs.name),
    );
    assert_eq!(
        text,
        format!("/Artifact /{p} BDC\n/{g} gs\nEMC\n/A#20b#2Fc /{p} BDC\nEMC\n")
    );
    assert_eq!(
        (props.kind, &props.res),
        (ResKind::Properties, &Res::dict(list("Header")))
    );
    assert_eq!(
        gs.res,
        Res::dict(dictionary! {
            "Type" => "ExtGState", "ca" => 0.5, "CA" => 1.0, "BM" => "Multiply",
        })
    );
    assert!(!text.contains("<<"));
}

/// Names come from the digest, bumped past what the resources bind to
/// something else, and are given where the caller gives them.
#[test]
fn names_are_bound_at_the_end() {
    let draw = |w: &mut ContentWriter| {
        w.begin_marked("Artifact", list("Header"));
        w.end_marked();
        w.begin_marked("Artifact", list("Footer"));
        w.end_marked();
    };
    let mut w = ContentWriter::new();
    draw(&mut w);
    let (_, needs) = written(w);
    let [header, footer] = &needs.0[..] else {
        panic!("{needs:?}")
    };
    for name in [&header.name, &footer.name] {
        assert_eq!(name.len(), 8, "{name:?}");
        assert!(name.starts_with(b"Sh"));
        assert!(
            name[2..]
                .iter()
                .all(|c| b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567".contains(c))
        );
    }
    assert_ne!(header.name, footer.name);

    // The header's name, taken by something else in the page's
    // resources: the next name up. Bound to the header's own list, it
    // stays.
    let mut pdf = Pdf::with_version("1.7");
    let same = pdf.add_object(list("Header"));
    let taken = |bound: Object| {
        let resources: Object = dictionary! {
            "Properties" => dictionary! { header.name.clone() => bound },
        }
        .into();
        Reserved::read(&pdf, Some(&resources))
    };
    for (bound, bumped) in [
        (list("Other").into(), true),
        (Object::Reference(same), false),
        (list("Header").into(), false),
        (Object::Reference((99, 0)), true),
    ] {
        let mut w = ContentWriter::new();
        draw(&mut w);
        let (_, needs) = w.finish(&taken(bound.clone()), &[]);
        let want = if bumped {
            next(&header.name)
        } else {
            header.name.clone()
        };
        assert_eq!(needs.0[0].name, want, "{bound:?}");
        assert_eq!(needs.0[1].name, footer.name);
    }
    // A name in another category doesn't count.
    let fonts: Object = dictionary! {
        "Font" => dictionary! { header.name.clone() => list("Other") },
    }
    .into();
    let mut w = ContentWriter::new();
    draw(&mut w);
    let (_, needs) = w.finish(&Reserved::read(&pdf, Some(&fonts)), &[]);
    assert_eq!(needs.0[0].name, header.name);

    // Given names are used as given, and a name given is never made.
    let mut w = ContentWriter::new();
    draw(&mut w);
    let ids = [header.res.id, footer.res.id];
    let given = [(ids[1], std::str::from_utf8(&header.name).unwrap())];
    let (_, needs) = w.finish(&Reserved::default(), &given);
    assert_eq!(needs.0[1].name, header.name);
    assert_eq!(needs.0[0].name, next(&header.name));
    let given = [(ids[0], "SheafHF")];
    let mut w = ContentWriter::new();
    draw(&mut w);
    let (bytes, needs) = w.finish(&Reserved::default(), &given);
    assert_eq!(needs.0[0].name, b"SheafHF");
    assert!(bytes.starts_with(b"/Artifact /SheafHF BDC\n"));
}

/// A resource drawn again is one need, where it was first drawn; a form
/// written already is named by its number.
#[test]
fn needs_are_one_a_resource_in_first_use_order() {
    let mut w = ContentWriter::new();
    w.font(&Std14::Helvetica, 9.0);
    w.draw(Res::written((7, 0)));
    w.font(&Std14::ZapfDingbats, 9.0);
    w.font(&Std14::Helvetica, 10.0);
    w.draw(Res::written((7, 0)));
    w.draw(Res::written((8, 0)));
    let (text, needs) = written(w);
    let kinds: Vec<_> = needs.0.iter().map(|n| (n.kind, &n.res.body)).collect();
    let font = |f: Std14| ResBody::Objects(f.resource().objects);
    assert_eq!(
        kinds,
        [
            (ResKind::Font, &font(Std14::Helvetica)),
            (ResKind::XObject, &ResBody::Written((7, 0))),
            (ResKind::Font, &font(Std14::ZapfDingbats)),
            (ResKind::XObject, &ResBody::Written((8, 0))),
        ]
    );
    let name = |i: usize| String::from_utf8_lossy(&needs.0[i].name).into_owned();
    assert_eq!(
        text,
        format!(
            "/{0} 9 Tf\n/{1} Do\n/{2} 9 Tf\n/{0} 10 Tf\n/{1} Do\n/{3} Do\n",
            name(0),
            name(1),
            name(2),
            name(3)
        )
    );
}

/// A form is placed by its box as its matrix turns it: a page turned a
/// quarter, its box off the origin, fills the target.
#[test]
fn place_form_honors_bbox_and_matrix() {
    let place = |bbox: Rect, matrix: Affine| {
        let mut w = ContentWriter::new();
        let target = Box2 {
            x: 100.0,
            y: 200.0,
            w: 50.0,
            h: 200.0,
        };
        w.place_form(Res::written((5, 0)), bbox, matrix, target);
        let (text, needs) = written(w);
        match needs.0.first() {
            Some(form) => text.replace(&*String::from_utf8_lossy(&form.name), "F"),
            None => text,
        }
    };
    // 200 × 100 at (50, 50), as it is: half as wide, twice as tall.
    let bbox = Rect::new(50.0, 50.0, 250.0, 150.0);
    assert_eq!(
        place(bbox, Affine::IDENTITY),
        "q\n0.25 0 0 2 87.5 100 cm\n/F Do\nQ\n"
    );
    // Turned a quarter from the origin: (x, y) is (-y, x), so it's
    // 100 × 200 at (-150, 50), drawn at a half.
    let turned = Affine {
        a: 0.0,
        b: 1.0,
        c: -1.0,
        d: 0.0,
        ..Affine::IDENTITY
    };
    assert_eq!(place(bbox, turned), "q\n0.5 0 0 1 175 150 cm\n/F Do\nQ\n");
    // A box with no area isn't drawn.
    assert_eq!(place(Rect::new(0.0, 0.0, 0.0, 9.0), Affine::IDENTITY), "");
    // An image fills its target.
    let mut w = ContentWriter::new();
    w.image(
        Res::written((6, 0)),
        Box2 {
            x: 1.0,
            y: 2.0,
            w: 3.0,
            h: 4.0,
        },
    );
    let (text, needs) = written(w);
    let name = String::from_utf8_lossy(&needs.0[0].name);
    assert_eq!(text, format!("q\n3 0 0 4 1 2 cm\n/{name} Do\nQ\n"));
}

/// The same calls give the same bytes and names, here and on other
/// threads.
#[test]
fn identical_calls_give_identical_bytes() {
    fn draw() -> (Vec<u8>, ResourceNeeds) {
        let mut w = ContentWriter::new();
        w.begin_marked("Artifact", list("Footer"));
        w.save();
        w.gstate(GState {
            fill_alpha: Some(0.3),
            ..GState::default()
        });
        w.begin_text();
        w.fill_color(Color::rgb8([200, 30, 30]));
        w.font(&Std14::HelveticaBold, 72.0);
        let turn = 45f32.to_radians();
        w.text_matrix(Affine {
            a: turn.cos(),
            b: turn.sin(),
            c: -turn.sin(),
            d: turn.cos(),
            e: 123.456,
            f: 789.012,
        });
        w.text("Café “x” – €");
        w.font(&Glyphless, 10.0);
        w.text("中文");
        w.end_text();
        w.restore();
        w.end_marked();
        w.finish(&Reserved::default(), &[])
    }
    let first = draw();
    assert_eq!(draw(), first);
    let threads: Vec<_> = (0..2).map(|_| std::thread::spawn(draw)).collect();
    for t in threads {
        assert_eq!(t.join().unwrap(), first);
    }
}

/// The needs written into a document once a save, and bound in each
/// resources dict: a category by reference is copied first.
#[test]
fn needs_are_written_once_and_bound() {
    let mut w = ContentWriter::new();
    w.font(&Glyphless, 10.0);
    w.begin_marked("Artifact", list("Header"));
    w.end_marked();
    let (_, needs) = written(w);
    let mut pdf = Pdf::with_version("1.7");
    let shared_fonts = pdf.add_object(dictionary! { "F1" => Object::Reference((1, 0)) });
    let mut res = ResObjects::default();
    let mut first = dictionary! { "Font" => shared_fonts };
    let mut second = Dictionary::new();
    res.bind(&mut pdf, &needs, &mut first);
    let count = pdf.objects.len();
    res.bind(&mut pdf, &needs, &mut second);
    assert_eq!(pdf.objects.len(), count, "nothing written twice");
    assert_eq!(first.get(b"Font").unwrap().as_dict().unwrap().len(), 2);
    assert_eq!(
        pdf.get_dictionary(shared_fonts).unwrap().len(),
        1,
        "the shared dict is left alone"
    );
    for key in [&b"Font"[..], b"Properties"] {
        let category = |r: &Dictionary| r.get(key).unwrap().as_dict().unwrap().clone();
        let [font, list] = &needs.0[..] else { panic!() };
        let name = if key == b"Font" {
            &font.name
        } else {
            &list.name
        };
        assert_eq!(
            category(&first).get(name).unwrap(),
            category(&second).get(name).unwrap()
        );
    }
    // Glyphless is its six objects, its program compressed.
    let font = res.object(&mut pdf, &Glyphless.resource().into());
    let type0 = pdf.get_dictionary(font).unwrap();
    let to_unicode = type0.get(b"ToUnicode").unwrap().as_reference().unwrap();
    let cmap = pdf.get_object(to_unicode).unwrap().as_stream().unwrap();
    assert!(
        cmap.decompressed_content()
            .unwrap()
            .windows(21)
            .any(|w| w == b"<0000> <FFFF> <0000>\n")
    );
    // A stream once a save.
    let q = res.stream(&mut pdf, dictionary! {}, b"q\n".to_vec());
    assert_eq!(res.stream(&mut pdf, dictionary! {}, b"q\n".to_vec()), q);
    let q = pdf.get_object(q).unwrap().as_stream().unwrap();
    assert_eq!(q.content, b"q\n");
}
