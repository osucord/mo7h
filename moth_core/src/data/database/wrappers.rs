use serenity::all::{GenericChannelId, MessageId, UserId};

macro_rules! id_wrapper {
    ($wrapper_name:ident, $maybe_name:ident, $inner_name:ident) => {
        #[derive(Clone, Copy, PartialEq, Debug)]
        pub struct $wrapper_name(pub $inner_name);

        impl std::ops::Deref for $wrapper_name {
            type Target = $inner_name;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        impl From<i64> for $wrapper_name {
            fn from(item: i64) -> Self {
                $wrapper_name($inner_name::new(item as u64))
            }
        }

        #[derive(Clone, Copy, PartialEq, Debug)]
        pub struct $maybe_name(pub Option<$wrapper_name>);

        impl $maybe_name {
            #[must_use]
            pub fn new(option: Option<$wrapper_name>) -> Self {
                $maybe_name(option)
            }
        }

        impl From<Option<i64>> for $maybe_name {
            fn from(option: Option<i64>) -> Self {
                $maybe_name(option.map($wrapper_name::from))
            }
        }

        impl std::ops::Deref for $maybe_name {
            type Target = Option<$wrapper_name>;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }
    };
}

id_wrapper!(UserIdWrapper, MaybeUserIdWrapper, UserId);
id_wrapper!(ChannelIdWrapper, MaybeChannelIdWrapper, GenericChannelId);
id_wrapper!(MessageIdWrapper, MaybeMessageIdWrapper, MessageId);
