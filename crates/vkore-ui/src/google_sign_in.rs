//! Google's unmodified neutral button, including its own lettering.
//! Only this bundled PNG is decoded; remote images never enter this path.
use crate::{Message, UiElement, account, typography::Typography};
use iced_core::image::Handle;
use std::{io::Cursor, sync::OnceLock};

fn artwork() -> Handle {
    static IMAGE: OnceLock<Handle> = OnceLock::new();
    IMAGE
        .get_or_init(|| {
            let bytes = include_bytes!("../assets/google/sign-in-neutral.png");
            let mut decoder = png::Decoder::new(Cursor::new(bytes));
            decoder.set_transformations(png::Transformations::normalize_to_color8());
            let mut reader = decoder.read_info().expect("bundled Google PNG header");
            assert_eq!((reader.info().width, reader.info().height), (720, 160));
            let mut pixels = vec![0; reader.output_buffer_size().expect("bounded PNG")];
            let info = reader.next_frame(&mut pixels).expect("bundled Google PNG");
            assert_eq!(info.color_type, png::ColorType::Rgba);
            pixels.truncate(info.buffer_size());
            Handle::from_rgba(info.width, info.height, pixels)
        })
        .clone()
}

pub fn button(ty: Typography) -> UiElement<'static> {
    // Scale both axes together, including for the user's text-size preference.
    iced_widget::button(
        iced_widget::image(artwork())
            .width(ty.size(180.))
            .height(ty.size(40.)),
    )
    .padding(0)
    .style(iced_widget::button::text)
    .on_press(Message::Account(account::Action::SignIn))
    .into()
}
