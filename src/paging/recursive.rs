use super::frame_alloc::*;
use super::page_table::*;
use addr::*;

pub trait Mapper {
    /// Creates a new mapping in the page table.
    ///
    /// This function might need additional physical frames to create new page tables. These
    /// frames are allocated from the `allocator` argument. At most three frames are required.
    fn map_to<A>(&mut self, page: Page, frame: Frame, flags: PageTableFlags, allocator: &mut A) -> Result<MapperFlush, MapToError>
        where A: FrameAllocator;

    /// Removes a mapping from the page table and returns the frame that used to be mapped.
    ///
    /// Note that no page tables or pages are deallocated.
    fn unmap(&mut self, page: Page) -> Result<(Frame, MapperFlush), UnmapError>;

    /// Return the frame that the specified page is mapped to.
    fn translate_page(&self, page: Page) -> Option<Frame>;

    /// Maps the given frame to the virtual page with the same address.
    fn identity_map<A>(&mut self, frame: Frame, flags: PageTableFlags, allocator: &mut A) -> Result<MapperFlush, MapToError>
        where A: FrameAllocator,
    {
        let page = Page::of_addr(VirtAddr::new(frame.start_address().as_u32() as usize));
        self.map_to(page, frame, flags, allocator)
    }
}

#[must_use = "Page Table changes must be flushed or ignored."]
pub struct MapperFlush(Page);

impl MapperFlush {
    /// Create a new flush promise
    fn new(page: Page) -> Self {
        MapperFlush(page)
    }

    /// Flush the page from the TLB to ensure that the newest mapping is used.
    pub fn flush(self) {
        use asm::sfence_vma;
        sfence_vma(0, self.0.start_address());
    }

    /// Don't flush the TLB and silence the “must be used” warning.
    pub fn ignore(self) {}
}

/// This error is returned from `map_to` and similar methods.
#[derive(Debug)]
pub enum MapToError {
    /// An additional frame was needed for the mapping process, but the frame allocator
    /// returned `None`.
    FrameAllocationFailed,
    /// An upper level page table entry has the `HUGE_PAGE` flag set, which means that the
    /// given page is part of an already mapped huge page.
    ParentEntryHugePage,
    /// The given page is already mapped to a physical frame.
    PageAlreadyMapped,
}

/// An error indicating that an `unmap` call failed.
#[derive(Debug)]
pub enum UnmapError {
    /// An upper level page table entry has the `HUGE_PAGE` flag set, which means that the
    /// given page is part of a huge page and can't be freed individually.
    ParentEntryHugePage,
    /// The given page is not mapped to a physical frame.
    PageNotMapped,
    /// The page table entry for the given page points to an invalid physical address.
    InvalidFrameAddress(PhysAddr),
}

/// A recursive page table is a last level page table with an entry mapped to the table itself.
///
/// This struct implements the `Mapper` trait.
pub struct RecursivePageTable<'a> {
    p2: &'a mut PageTable,
    recursive_index: usize,
}

/// An error indicating that the given page table is not recursively mapped.
///
/// Returned from `RecursivePageTable::new`.
#[derive(Debug)]
pub struct NotRecursivelyMapped;

impl<'a> RecursivePageTable<'a> {
    /// Creates a new RecursivePageTable from the passed level 2 PageTable.
    ///
    /// The page table must be recursively mapped, that means:
    ///
    /// - The page table must have one recursive entry, i.e. an entry that points to the table
    ///   itself.
    /// - The page table must be active, i.e. the satp register must contain its physical address.
    ///
    /// Otherwise `Err(NotRecursivelyMapped)` is returned.
    pub fn new(table: &'a mut PageTable) -> Result<Self, NotRecursivelyMapped> {
        let page = Page::of_addr(VirtAddr::new(table as *const _ as usize));
        let recursive_index = page.p2_index();

        if page.p1_index() != recursive_index {
            return Err(NotRecursivelyMapped);
        }
        use register::satp;
        if satp::read().frame() != table[recursive_index].frame() {
            return Err(NotRecursivelyMapped);
        }

        Ok(RecursivePageTable {
            p2: table,
            recursive_index,
        })
    }

    /// Creates a new RecursivePageTable without performing any checks.
    ///
    /// The `recursive_index` parameter must be the index of the recursively mapped entry.
    pub unsafe fn new_unchecked(table: &'a mut PageTable, recursive_index: usize) -> Self {
        RecursivePageTable {
            p2: table,
            recursive_index,
        }
    }

    /// Internal helper function to create the page table of the next level if needed.
    ///
    /// If the passed entry is unused, a new frame is allocated from the given allocator, zeroed,
    /// and the entry is updated to that address. If the passed entry is already mapped, the next
    /// table is returned directly.
    ///
    /// The `next_page_table` page must be the page of the next page table in the hierarchy.
    ///
    /// Returns `MapToError::FrameAllocationFailed` if the entry is unused and the allocator
    /// returned `None`.
    unsafe fn create_next_table<'b, A>(
        entry: &'b mut PageTableEntry,
        next_table_page: Page,
        allocator: &mut A,
    ) -> Result<&'b mut PageTable, MapToError>
        where A: FrameAllocator,
    {
        /// This inner function is used to limit the scope of `unsafe`.
        ///
        /// This is a safe function, so we need to use `unsafe` blocks when we do something unsafe.
        #[inline(always)]
        fn inner<'b, A>(
            entry: &'b mut PageTableEntry,
            next_table_page: Page,
            allocator: &mut A,
        ) -> Result<&'b mut PageTable, MapToError>
            where A: FrameAllocator,
        {
            use self::PageTableFlags as Flags;

            let created = if entry.is_unused() {
                if let Some(frame) = allocator.alloc() {
                    entry.set(frame, Flags::VALID);
                } else {
                    return Err(MapToError::FrameAllocationFailed);
                }
                true
            } else {
                false
            };

            let page_table = unsafe { next_table_page.start_address().as_mut::<PageTable>() };
            if created {
                page_table.zero();
            }
            Ok(page_table)
        }

        inner(entry, next_table_page, allocator)
    }
}

impl<'a> Mapper for RecursivePageTable<'a> {
    fn map_to<A>(&mut self, page: Page, frame: Frame, flags: PageTableFlags, allocator: &mut A) -> Result<MapperFlush, MapToError>
        where A: FrameAllocator,
    {
        use self::PageTableFlags as Flags;
        let p1_page = p1_page(page, self.recursive_index);
        let p1 = unsafe { Self::create_next_table(&mut self.p2[page.p2_index()], p1_page, allocator)? };

        if !p1[page.p1_index()].is_unused() {
            return Err(MapToError::PageAlreadyMapped);
        }
        p1[page.p1_index()].set(frame, flags);
        Ok(MapperFlush::new(page))
    }

    fn unmap(&mut self, page: Page) -> Result<(Frame, MapperFlush), UnmapError> {
        use self::PageTableFlags as Flags;
        let p2_entry = &self.p2[page.p2_index()];
        if p2_entry.is_unused() {
            return Err(UnmapError::PageNotMapped);
        }
        let p1 = unsafe { p1_page(page, self.recursive_index).start_address().as_mut::<PageTable>() };
        let p1_entry = &mut p1[page.p1_index()];
        if !p1_entry.flags().contains(Flags::VALID) {
            return Err(UnmapError::PageNotMapped);
        }
        let frame = p1_entry.frame();
        p1_entry.set_unused();
        Ok((frame, MapperFlush::new(page)))
    }

    fn translate_page(&self, page: Page) -> Option<Frame> {
        if self.p2[page.p2_index()].is_unused() {
            return None;
        }
        let p1 = unsafe { p1_page(page, self.recursive_index).start_address().as_mut::<PageTable>() };
        let p1_entry = &p1[page.p1_index()];
        if p1_entry.is_unused() {
            return None;
        }
        Some(p1_entry.frame())
    }
}

fn p1_page(page: Page, recursive_index: usize) -> Page {
    Page::from_page_table_indices(recursive_index, page.p2_index())
}