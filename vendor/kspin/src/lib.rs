#![cfg_attr(not(test), no_std)]
#![doc = include_str!("../README.md")]

mod base;

pub use kernel_guard::{IrqSave, NoPreemptIrqSave};
use kernel_guard::{NoOp, NoPreempt};

pub use self::base::{BaseSpinLock, BaseSpinLockGuard};

/// A spin lock that disables local IRQs while trying to lock, and restores
/// their previous state after unlocking.
///
/// It is suitable for short critical sections that cannot schedule while IRQs
/// are disabled and therefore do not need a separate preemption disable count.
pub type SpinIrq<T> = BaseSpinLock<IrqSave, T>;

/// A guard that provides mutable data access for [`SpinIrq`].
pub type SpinIrqGuard<'a, T> = BaseSpinLockGuard<'a, IrqSave, T>;

/// A spin lock that disables kernel preemption while trying to lock, and
/// re-enables it after unlocking.
///
/// It must be used in the local IRQ-disabled context, or never be used in
/// interrupt handlers.
pub type SpinNoPreempt<T> = BaseSpinLock<NoPreempt, T>;

/// A guard that provides mutable data access for [`SpinNoPreempt`].
pub type SpinNoPreemptGuard<'a, T> = BaseSpinLockGuard<'a, NoPreempt, T>;

/// A spin lock that disables kernel preemption and local IRQs while trying to
/// lock, and re-enables it after unlocking.
///
/// It can be used in the IRQ-enabled context.
pub type SpinNoIrq<T> = BaseSpinLock<NoPreemptIrqSave, T>;

/// A guard that provides mutable data access for [`SpinNoIrq`].
pub type SpinNoIrqGuard<'a, T> = BaseSpinLockGuard<'a, NoPreemptIrqSave, T>;

/// A raw spin lock that does nothing while trying to lock.
///
/// It must be used in the preemption-disabled and local IRQ-disabled context,
/// or never be used in interrupt handlers.
pub type SpinRaw<T> = BaseSpinLock<NoOp, T>;

/// A guard that provides mutable data access for [`SpinRaw`].
pub type SpinRawGuard<'a, T> = BaseSpinLockGuard<'a, NoOp, T>;
