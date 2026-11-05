//! Send desktop events through the firmware's own PEG queue.
//!
//! A small ARM trampoline preserves the interrupted task's registers and flags.
//! Calls are deferred during interrupts, kernel critical sections and small stacks.

use crate::{
    display::valid,
    unicorn::{CPSR, Cpu, PC, R0, SP},
};
use std::collections::VecDeque;

const MESSAGE_SIZE: usize = 64;
const MAX_PENDING: usize = 64;
const RETRY_DELAY: u64 = 100_000;
const MENU_EVENT: u32 = 0x5006;

// RED 6.3.106 native addresses. The scratch area is reserved in IRAM.
const TRAMPOLINE: u32 = 0x4020fb00;
const CALL_RETURN: u32 = TRAMPOLINE + 0x1c;
const RESTORE_CALLER: u32 = TRAMPOLINE + 0x20;
const MESSAGE_BUFFER: u32 = 0x4020f800;
const MENU_CALLBACK: u32 = 0x4020f600;
const PEG_QUEUE_POST: u32 = 0x80a9e148;
const ASYNC_RUN: u32 = 0x8007fbfc;
const CURRENT_THREAD: u32 = 0x810da120;
const SYSTEM_STATE: u32 = 0x810d9b7c;
const PREEMPT_DISABLE: u32 = 0x810daa54;
const TIMER_THREAD: u32 = 0x810da2b0;
const PEG_THREAD: u32 = 0x824d63d8;
const PEG_RECEIVER: u32 = 0x825de90c;
const UI_MANAGER: u32 = 0x824d3e40;
const THREAD_MAGIC: u32 = 0x54485244;

type Message = [u8; MESSAGE_SIZE];

#[derive(Debug, Clone)]
pub enum Input {
    Down(i16, i16),
    Up(i16, i16),
    Move(i16, i16),
    KeyDown(u16),
    KeyUp(u16),
    Menu,
}

impl Input {
    pub fn encode(&self) -> Message {
        let mut message = [0; MESSAGE_SIZE];
        let kind: u32 = match self {
            Self::Down(..) => 10,
            Self::Up(..) => 11,
            Self::Move(..) => 9,
            Self::KeyDown(_) => 0x21,
            Self::KeyUp(_) => 0x22,
            Self::Menu => MENU_EVENT,
        };
        message[8..12].copy_from_slice(&kind.to_le_bytes());

        match self {
            Self::Down(x, y) | Self::Up(x, y) | Self::Move(x, y) => {
                message[16..18].copy_from_slice(&x.to_le_bytes());
                message[18..20].copy_from_slice(&y.to_le_bytes());
            }
            Self::KeyDown(key) | Self::KeyUp(key) => {
                message[12..14].copy_from_slice(&key.to_le_bytes());
            }
            Self::Menu => {}
        }
        message
    }
}

struct Flight {
    pc: u32,
    sp: u32,
    thread: u32,
    message: Message,
}

#[derive(Default)]
pub struct NativeInput {
    queue: VecDeque<Message>,
    flight: Option<Flight>,
    retry_after: u64,
    pub delivered: u64,
    pub rejected: u64,
}

impl NativeInput {
    pub fn is_idle(&self) -> bool {
        self.queue.is_empty() && self.flight.is_none()
    }

    pub fn enqueue(&mut self, event: Input) {
        // Keep button/key edges, but replace a stale consecutive drag position.
        let previous_is_move = self.queue.back().is_some_and(|message| message[8] == 9);
        if matches!(event, Input::Move(..)) && previous_is_move {
            self.queue.pop_back();
        }
        if self.queue.len() < MAX_PENDING {
            self.queue.push_back(event.encode());
        }
    }

    pub fn dispatch(
        &mut self,
        cpu: Cpu,
        pc: u32,
        instructions: u64,
        offline: bool,
        async_launch: &mut bool,
    ) -> bool {
        if self.flight.is_some() {
            return self.resume_in_flight(cpu, pc, instructions, async_launch);
        }
        if self.queue.is_empty() || instructions < self.retry_after {
            return false;
        }
        let Some(thread) = Self::eligible_thread(cpu, pc) else {
            return false;
        };
        let message = *self.queue.front().expect("pending event checked above");
        let is_menu = message_kind(&message) == MENU_EVENT;
        let sp = cpu.reg(SP);
        if !Self::has_stack_space(cpu, thread, sp, is_menu) {
            return false;
        }

        let (target, argument0, argument1) = if is_menu && offline {
            let manager = cpu.word(UI_MANAGER);
            if manager == 0 || cpu.word(manager + 0x14) == 0 {
                return false;
            }
            cpu.put(MENU_CALLBACK, 0xe1a00000);
            *async_launch = true;
            (ASYNC_RUN, MENU_CALLBACK, 0)
        } else {
            (PEG_QUEUE_POST, cpu.word(PEG_RECEIVER), MESSAGE_BUFFER)
        };

        self.queue.pop_front();
        let _ = cpu.write(MESSAGE_BUFFER, &message);
        Self::write_trampoline(cpu, argument0, argument1, target, pc);
        self.flight = Some(Flight {
            pc,
            sp,
            thread,
            message,
        });
        cpu.set(PC, TRAMPOLINE);
        true
    }

    fn resume_in_flight(
        &mut self,
        cpu: Cpu,
        pc: u32,
        instructions: u64,
        async_launch: &mut bool,
    ) -> bool {
        let flight = self
            .flight
            .as_ref()
            .expect("active call checked by dispatch");
        if pc == CALL_RETURN {
            if cpu.reg(R0) != 0 {
                self.delivered += 1;
            } else {
                self.queue.push_front(flight.message);
                self.rejected += 1;
                self.retry_after = instructions + RETRY_DELAY;
            }
            if message_kind(&flight.message) == MENU_EVENT {
                *async_launch = false;
            }
            cpu.set(PC, RESTORE_CALLER);
            return true;
        }

        let caller_resumed = pc == flight.pc
            && cpu.reg(SP) == flight.sp
            && cpu.word(CURRENT_THREAD) == flight.thread;
        if caller_resumed {
            self.flight = None;
            // Run the interrupted block once before posting the next event.
            return true;
        }
        false
    }

    fn eligible_thread(cpu: Cpu, pc: u32) -> Option<u32> {
        if !(0x80000000..0x810d0000).contains(&pc) {
            return None;
        }
        let flags = cpu.reg(CPSR);
        let interrupts_masked_or_thumb = flags & 0xa0 != 0;
        let eligible_mode = [0x13, 0x1f].contains(&(flags & 31));
        if interrupts_masked_or_thumb
            || !eligible_mode
            || cpu.word(SYSTEM_STATE) != 0
            || cpu.word(PREEMPT_DISABLE) != 0
        {
            return None;
        }

        let thread = cpu.word(CURRENT_THREAD);
        let thread_ready =
            valid(thread, 0x34) && cpu.word(thread) == THREAD_MAGIC && thread != TIMER_THREAD;
        let queue_ready = cpu.word(PEG_THREAD) == THREAD_MAGIC && cpu.word(PEG_RECEIVER) != 0;
        (thread_ready && queue_ready).then_some(thread)
    }

    fn has_stack_space(cpu: Cpu, thread: u32, sp: u32, is_menu: bool) -> bool {
        let headroom = if is_menu { 8192 } else { 1024 };
        let Some(bottom) = sp.checked_sub(headroom) else {
            return false;
        };
        sp & 7 == 0
            && bottom >= cpu.word(thread + 0xc)
            && sp <= cpu.word(thread + 0x10).saturating_add(1)
            && valid(bottom, headroom as usize)
    }

    fn write_trampoline(cpu: Cpu, argument0: u32, argument1: u32, target: u32, resume_pc: u32) {
        cpu.bytes(
            TRAMPOLINE,
            &[
                0xe92d500f, // push {r0-r3, r12, lr}
                0xe10fc000, // mrs r12, cpsr
                0xe92d1010, // push {r4, r12}, preserving stack alignment
                0xe59f001c, // load argument0
                0xe59f101c, // load argument1
                0xe59fc01c, // load target
                0xe12fff3c, // blx r12
                0xeafffffe, // return hook; force a basic-block boundary
                0xe8bd1010, // pop {r4, r12}
                0xe12cf00c, // restore CPSR flags, including GE
                0xe8bd500f, // restore caller registers
                0xe59ff008, // resume original block via literal
                argument0, argument1, target, resume_pc,
            ],
        );
    }
}

fn message_kind(message: &Message) -> u32 {
    u32::from_le_bytes(message[8..12].try_into().expect("fixed message layout"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_message_layout() {
        let b = Input::Down(160, 85).encode();
        assert_eq!(&b[8..12], &10u32.to_le_bytes());
        assert_eq!(&b[16..20], &[160, 0, 85, 0]);
        let b = Input::KeyDown(0x295).encode();
        assert_eq!(&b[12..14], &[0x95, 2]);
    }

    #[test]
    fn moves_coalesce_but_edges_remain() {
        let mut q = NativeInput::default();
        q.enqueue(Input::Down(1, 2));
        q.enqueue(Input::Move(2, 2));
        q.enqueue(Input::Move(3, 2));
        q.enqueue(Input::Up(3, 2));
        assert_eq!(q.queue.len(), 3);
        assert_eq!(q.queue[1][16], 3);
    }
}
