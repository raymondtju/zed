use std::{
    fmt::Debug,
    marker::PhantomData,
    ops::{Add, AddAssign, Sub, SubAssign},
};
use text::{Point, PointUtf16};

#[repr(transparent)]
pub struct TypedOffset<T> {
    pub value: usize,
    _marker: PhantomData<T>,
}

#[repr(transparent)]
pub struct TypedPoint<T> {
    pub value: Point,
    _marker: PhantomData<T>,
}

#[repr(transparent)]
pub struct TypedPointUtf16<T> {
    pub value: PointUtf16,
    _marker: PhantomData<T>,
}

#[repr(transparent)]
pub struct TypedRow<T> {
    pub value: u32,
    _marker: PhantomData<T>,
}

impl<T> TypedOffset<T> {
    pub fn new(offset: usize) -> Self {
        Self {
            value: offset,
            _marker: PhantomData,
        }
    }
}
impl<T> TypedPoint<T> {
    pub fn new(row: u32, column: u32) -> Self {
        Self {
            value: Point::new(row, column),
            _marker: PhantomData,
        }
    }
    pub fn wrap(point: Point) -> Self {
        Self {
            value: point,
            _marker: PhantomData,
        }
    }
    pub fn row(&self) -> u32 {
        self.value.row
    }
    pub fn column(&self) -> u32 {
        self.value.column
    }
}
impl<T> TypedPointUtf16<T> {
    pub fn new(row: u32, column: u32) -> Self {
        TypedPointUtf16 {
            value: PointUtf16::new(row, column),
            _marker: PhantomData,
        }
    }
    pub fn wrap(point: PointUtf16) -> Self {
        Self {
            value: point,
            _marker: PhantomData,
        }
    }
}
impl<T> TypedRow<T> {
    pub fn new(row: u32) -> Self {
        Self {
            value: row,
            _marker: PhantomData,
        }
    }
}

impl<T> Copy for TypedOffset<T> {}
impl<T> Copy for TypedPoint<T> {}
impl<T> Copy for TypedPointUtf16<T> {}
impl<T> Copy for TypedRow<T> {}

impl<T> Clone for TypedOffset<T> {
    fn clone(&self) -> Self {
        Self {
            value: self.value,
            _marker: PhantomData,
        }
    }
}
impl<T> Clone for TypedPoint<T> {
    fn clone(&self) -> Self {
        Self {
            value: self.value,
            _marker: PhantomData,
        }
    }
}
impl<T> Clone for TypedPointUtf16<T> {
    fn clone(&self) -> Self {
        Self {
            value: self.value,
            _marker: PhantomData,
        }
    }
}
impl<T> Clone for TypedRow<T> {
    fn clone(&self) -> Self {
        Self {
            value: self.value,
            _marker: PhantomData,
        }
    }
}

impl<T> Default for TypedOffset<T> {
    fn default() -> Self {
        Self::new(0)
    }
}
impl<T> Default for TypedPoint<T> {
    fn default() -> Self {
        Self::wrap(Point::default())
    }
}
impl<T> Default for TypedPointUtf16<T> {
    fn default() -> Self {
        Self::wrap(PointUtf16::default())
    }
}
impl<T> Default for TypedRow<T> {
    fn default() -> Self {
        Self::new(0)
    }
}

impl<T> PartialOrd for TypedOffset<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.value.cmp(&other.value))
    }
}
impl<T> PartialOrd for TypedPoint<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.value.cmp(&other.value))
    }
}
impl<T> PartialOrd for TypedPointUtf16<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.value.cmp(&other.value))
    }
}
impl<T> PartialOrd for TypedRow<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.value.cmp(&other.value))
    }
}

impl<T> Ord for TypedOffset<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.value.cmp(&other.value)
    }
}
impl<T> Ord for TypedPoint<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.value.cmp(&other.value)
    }
}
impl<T> Ord for TypedPointUtf16<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.value.cmp(&other.value)
    }
}
impl<T> Ord for TypedRow<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.value.cmp(&other.value)
    }
}

impl<T> PartialEq for TypedOffset<T> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}
impl<T> PartialEq for TypedPoint<T> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}
impl<T> PartialEq for TypedPointUtf16<T> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}
impl<T> PartialEq for TypedRow<T> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl<T> Eq for TypedOffset<T> {}
impl<T> Eq for TypedPoint<T> {}
impl<T> Eq for TypedPointUtf16<T> {}
impl<T> Eq for TypedRow<T> {}

impl<T> Debug for TypedOffset<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}Offset({})", type_name::<T>(), self.value)
    }
}
impl<T> Debug for TypedPoint<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}Point({}, {})",
            type_name::<T>(),
            self.value.row,
            self.value.column
        )
    }
}
impl<T> Debug for TypedRow<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}Row({})", type_name::<T>(), self.value)
    }
}

fn type_name<T>() -> &'static str {
    std::any::type_name::<T>().split("::").last().unwrap()
}

impl<T> Add<TypedOffset<T>> for TypedOffset<T> {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        TypedOffset::new(self.value + other.value)
    }
}
impl<T> Add<TypedPoint<T>> for TypedPoint<T> {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        TypedPoint::wrap(self.value + other.value)
    }
}

impl<T> Sub<TypedOffset<T>> for TypedOffset<T> {
    type Output = Self;
    fn sub(self, other: Self) -> Self {
        TypedOffset::new(self.value - other.value)
    }
}
impl<T> Sub<TypedPoint<T>> for TypedPoint<T> {
    type Output = Self;
    fn sub(self, other: Self) -> Self {
        TypedPoint::wrap(self.value - other.value)
    }
}

impl<T> AddAssign<TypedOffset<T>> for TypedOffset<T> {
    fn add_assign(&mut self, other: Self) {
        self.value += other.value;
    }
}
impl<T> AddAssign<TypedPoint<T>> for TypedPoint<T> {
    fn add_assign(&mut self, other: Self) {
        self.value += other.value;
    }
}

impl<T> SubAssign<Self> for TypedOffset<T> {
    fn sub_assign(&mut self, other: Self) {
        self.value -= other.value;
    }
}
impl<T> SubAssign<Self> for TypedRow<T> {
    fn sub_assign(&mut self, other: Self) {
        self.value -= other.value;
    }
}
