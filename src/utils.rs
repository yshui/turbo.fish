use std::borrow::Cow;

pub(crate) trait CowEx<T: ?Sized + ToOwned> {
    fn to_static(self) -> Cow<'static, T>;
}

impl<T: ?Sized + ToOwned> CowEx<T> for Cow<'_, T> {
    fn to_static(self) -> Cow<'static, T> {
        match self {
            Cow::Borrowed(b) => Cow::Owned(b.to_owned()),
            Cow::Owned(o) => Cow::Owned(o),
        }
    }
}
