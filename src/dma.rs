//! Interface to the DMA peripheral
//!
//! See STM32L0x2 Reference Manual, chapter 11.


use core::{
    fmt,
    ops::{
        Deref,
        DerefMut,
    },
    pin::Pin,
    sync::atomic::{
        compiler_fence,
        Ordering,
    }
};

use as_slice::{
    AsMutSlice,
    AsSlice,
};

use crate::{
    pac::{
        self,
        dma1::ccr1,
    },
    rcc::Rcc,
};


/// Entry point to the DMA API
pub struct DMA {
    /// Handle to the DMA peripheral
    pub handle: Handle,

    /// DMA channels
    pub channels: Channels
}

impl DMA {
    /// Create an instance of the DMA API
    pub fn new(dma: pac::DMA1, rcc: &mut Rcc) -> Self {
        // Reset peripheral
        rcc.rb.ahbrstr.modify(|_, w| w.dmarst().set_bit());
        rcc.rb.ahbrstr.modify(|_, w| w.dmarst().clear_bit());

        // Enable peripheral clock
        rcc.rb.ahbenr.modify(|_, w| w.dmaen().set_bit());

        Self {
            handle:   Handle { dma },
            channels: Channels::new(),
        }
    }
}


/// Handle to the DMA peripheral
pub struct Handle {
    dma: pac::DMA1,
}


pub struct Transfer<C, T, B, State> {
    res:    TransferResources<C, T, B>,
    _state: State,
}

impl<T, C, B> Transfer<T, C, B, Ready>
    where
        T: Target<C>,
        C: Channel,
{
    pub(crate) fn memory_to_peripheral(
        handle:  &mut Handle,
        target:  T,
        channel: C,
        buffer:  Pin<B>,
        address: u32,
    )
        -> Self
        where
            B:         Deref,
            B::Target: AsSlice<Element=u8>,
    {
        // Safe, because the traits bounds of this method guarantee that
        // `buffer` can be read from.
        unsafe {
            Self::new(
                handle,
                target,
                channel,
                buffer,
                address,
                ccr1::DIRW::FROMMEMORY,
            )
        }
    }

    pub(crate) fn peripheral_to_memory(
        handle:  &mut Handle,
        target:  T,
        channel: C,
        buffer:  Pin<B>,
        address: u32,
    )
        -> Self
        where
            B:         DerefMut,
            B::Target: AsMutSlice<Element=u8>,
    {
        // Safe, because the traits bounds of this method guarantee that
        // `buffer` can be written to.
        unsafe {
            Self::new(
                handle,
                target,
                channel,
                buffer,
                address,
                ccr1::DIRW::FROMPERIPHERAL,
            )
        }
    }

    /// Internal constructor
    ///
    /// # Safety
    ///
    /// If this is used to prepare a memory-to-peripheral transfer, the caller
    /// must make sure that the buffer can be read from safely.
    ///
    /// If this is used to prepare a peripheral-to-memory transfer, the caller
    /// must make sure that the buffer can be written to safely.
    ///
    /// # Panics
    ///
    /// Panics, if the length of the buffer is larger than `u16::max_value()`.
    unsafe fn new(
        handle:  &mut Handle,
        target:  T,
        channel: C,
        buffer:  Pin<B>,
        address: u32,
        dir:     ccr1::DIRW,
    )
        -> Self
        where
            B:         Deref,
            B::Target: AsSlice<Element=u8>,
    {
        assert!(buffer.as_slice().len() <= u16::max_value() as usize);

        channel.select_target(handle, &target);
        channel.set_peripheral_address(handle, address);
        channel.set_memory_address(handle, buffer.as_slice().as_ptr() as u32);
        channel.set_transfer_len(handle, buffer.as_slice().len() as u16);
        channel.configure(handle, dir);

        Transfer {
            res: TransferResources {
                target,
                channel,
                buffer,
            },
            _state: Ready,
        }
    }

    pub fn start(self) -> Transfer<T, C, B, Started> {
        compiler_fence(Ordering::SeqCst);

        self.res.channel.start();

        Transfer {
            res:    self.res,
            _state: Started,
        }
    }
}

impl<T, C, B> Transfer<T, C, B, Started>
    where C: Channel
{
    /// Indicates whether the transfer is still ongoing
    pub fn is_active(&self) -> bool {
        self.res.channel.is_active()
    }

    /// Waits for the transfer to finish and return owned resources
    ///
    /// This function will busily wait until the transfer is finished. If you
    /// don't want this, please call this function only once you know that the
    /// transfer has finished.
    ///
    /// This function will return immediately, if [`Transfer::is_active`]
    /// returns `false`.
    pub fn wait(self)
        -> Result<
            TransferResources<T, C, B>,
            (TransferResources<T, C, B>, Error)
        >
    {
        while self.is_active() {
            if self.res.channel.error_occured() {
                return Err((self.res, Error));
            }
        }

        compiler_fence(Ordering::SeqCst);

        if self.res.channel.error_occured() {
            return Err((self.res, Error));
        }

        Ok(self.res)
    }
}


pub struct TransferResources<T, C, B> {
    pub target:  T,
    pub channel: C,
    pub buffer:  Pin<B>,
}

// Since `TransferResources` is used in the error variant of a `Result`, it
// needs to implement `Debug` for methods like `unwrap` to work. We can't just
// derive `Debug`, without requiring all type parameters to be `Debug`, which
// seems to restrictive.
impl<T, C, B> fmt::Debug for TransferResources<T, C, B> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "TransferResources {{ ... }}")
    }
}


#[derive(Debug)]
pub struct Error;


pub trait Channel: Sized {
    fn select_target<T: Target<Self>>(&self, _: &mut Handle, target: &T);
    fn set_peripheral_address(&self, _: &mut Handle, address: u32);
    fn set_memory_address(&self, _: &mut Handle, address: u32);
    fn set_transfer_len(&self, _: &mut Handle, len: u16);
    fn configure(&self, _: &mut Handle, dir: ccr1::DIRW);
    fn start(&self);
    fn is_active(&self) -> bool;
    fn error_occured(&self) -> bool;
}

macro_rules! impl_channel {
    (
        $(
            $channel:ident,
            $field:ident,
            $cxs:ident,
            $cpar:ident,
            $cmar:ident,
            $cndtr:ident,
            $ccr:ident,
            $tcif:ident,
            $teif:ident,
            $ctcif:ident,
            $cteif:ident;
        )*
    ) => {
        pub struct Channels {
            $(pub $field: $channel,)*
        }

        impl Channels {
            pub fn new() -> Self {
                Self {
                    $($field: $channel(()),)*
                }
            }
        }

        $(
            pub struct $channel(());

            impl Channel for $channel {
                fn select_target<T: Target<Self>>(&self,
                    handle:  &mut Handle,
                    _target: &T,
                ) {
                    handle.dma.cselr.modify(|_, w| w.$cxs().bits(T::REQUEST));
                }

                fn set_peripheral_address(&self,
                    handle:  &mut Handle,
                    address: u32,
                ) {
                    handle.dma.$cpar.write(|w| w.pa().bits(address));
                }

                fn set_memory_address(&self,
                    handle:  &mut Handle,
                    address: u32,
                ) {
                    handle.dma.$cmar.write(|w| w.ma().bits(address));
                }

                fn set_transfer_len(&self, handle: &mut Handle, len: u16) {
                    handle.dma.$cndtr.write(|w| w.ndt().bits(len));
                }

                fn configure(&self,
                    handle: &mut Handle,
                    dir:    ccr1::DIRW,
                ) {
                    // TASK: MSIZE and PSIZE are incorrect. Should be 32 bits.
                    handle.dma.$ccr.write(|w|
                        w
                            // Memory-to-memory mode disabled
                            .mem2mem().disabled()
                            // Low priority
                            .pl().low()
                            // Word size in memory
                            .msize().bit8()
                            // Word size in peripheral
                            .psize().bit8()
                            // Increment memory pointer
                            .minc().enabled()
                            // Don't increment peripheral pointer
                            .pinc().disabled()
                            // Circular mode disabled
                            .circ().disabled()
                            // Data transfer direction
                            .dir().bit(dir._bits())
                            // Disable interrupts
                            .teie().disabled()
                            .htie().disabled()
                            .tcie().disabled()
                    );
                }

                fn start(&self) {
                    // Safe, because we're only accessing a register that this
                    // channel has exclusive access to.
                    let ccr = &unsafe { &*pac::DMA1::ptr() }.$ccr;

                    // Start transfer
                    ccr.modify(|_, w| w.en().enabled());
                }

                fn is_active(&self) -> bool {
                    // This is safe, for the following reasons:
                    // - We only do one atomic read of ISR.
                    // - IFCR is a stateless register and we don one atomic
                    //   write.
                    // - This channel has exclusive access to CCRx.
                    let dma = unsafe { &*pac::DMA1::ptr() };

                    if dma.isr.read().$tcif().is_complete() {
                        dma.ifcr.write(|w| w.$ctcif().set_bit());
                        dma.$ccr.modify(|_, w| w.en().disabled());
                        false
                    }
                    else {
                        true
                    }
                }

                fn error_occured(&self) -> bool {
                    // This is safe, for the following reasons:
                    // - We only do one atomic read of ISR.
                    // - IFCR is a stateless register and we don one atomic
                    //   write.
                    let dma = unsafe { &*pac::DMA1::ptr() };

                    if dma.isr.read().$teif().is_error() {
                        dma.ifcr.write(|w| w.$cteif().set_bit());
                        true
                    }
                    else {
                        false
                    }
                }
            }
        )*
    }
}

impl_channel!(
    Channel1, channel1,
        c1s, cpar1, cmar1, cndtr1, ccr1,
        tcif1, teif1, ctcif1, cteif1;
    Channel2, channel2,
        c2s, cpar2, cmar2, cndtr2, ccr2,
        tcif2, teif2, ctcif2, cteif2;
    Channel3, channel3,
        c3s, cpar3, cmar3, cndtr3, ccr3,
        tcif3, teif3, ctcif3, cteif3;
    Channel4, channel4,
        c4s, cpar4, cmar4, cndtr4, ccr4,
        tcif4, teif4, ctcif4, cteif4;
    Channel5, channel5,
        c5s, cpar5, cmar5, cndtr5, ccr5,
        tcif5, teif5, ctcif5, cteif5;
    Channel6, channel6,
        c6s, cpar6, cmar6, cndtr6, ccr6,
        tcif6, teif6, ctcif6, cteif6;
    Channel7, channel7,
        c7s, cpar7, cmar7, cndtr7, ccr7,
        tcif7, teif7, ctcif7, cteif7;
);


pub trait Target<Channel> {
    const REQUEST: u8;
}

macro_rules! impl_target {
    ($($target:ty, $channel:ty, $request:expr;)*) => {
        $(
            impl Target<$channel> for $target {
                const REQUEST: u8 = $request;
            }
        )*
    }
}

// See STM32L0x2 Reference Manual, table 51 (page 267).
impl_target!();


/// Indicates that a DMA transfer is ready
pub struct Ready;

/// Indicates that a DMA transfer has been started
pub struct Started;
