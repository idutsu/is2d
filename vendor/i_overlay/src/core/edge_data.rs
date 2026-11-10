use crate::segm::boolean::ShapeCountBoolean;
use i_float::int::number::int::IntNumber;
use i_float::int::point::IntPoint;

pub trait OverlayEdgeData<C = ShapeCountBoolean>: Copy + PartialEq + Send + Sync {
    type Store: Default;

    #[inline(always)]
    fn reversed(self, _store: &mut Self::Store) -> Self {
        self
    }

    #[inline(always)]
    fn split<I: IntNumber>(self, _ctx: EdgeDataSplit<I>, _store: &mut Self::Store) -> (Self, Self) {
        (self, self)
    }

    fn merge(ctx: EdgeDataMerge<C, Self>, store: &mut Self::Store) -> Self;

    /// is2d で足したもの（PATCH.md）：重なった区間をまとめて数（count）が打ち消し合っても、区間を残すか
    #[inline(always)]
    fn keep_when_count_cancels(&self) -> bool {
        false
    }
}

#[derive(Debug, Clone)]
pub struct EdgeDataSplit<I: IntNumber> {
    pub a: IntPoint<I>,
    pub p: IntPoint<I>,
    pub b: IntPoint<I>,
}

#[derive(Debug, Clone)]
pub struct EdgeDataMerge<C, D> {
    pub lhs_data: D,
    pub lhs_count: C,
    pub rhs_data: D,
    pub rhs_count: C,
    pub out_count: C,
}

impl<C> OverlayEdgeData<C> for ()
where
    C: Send + Sync,
{
    type Store = ();

    #[inline(always)]
    fn merge(_: EdgeDataMerge<C, Self>, _: &mut Self::Store) -> Self {}
}
