pub const SIZEOF_INT: c_size_t = mem::size_of::<c_int>() as c_size_t;

pub const SIZEOF_SHIFT: c_size_t = (mem::size_of::<c_int>() as c_size_t) << 3;
