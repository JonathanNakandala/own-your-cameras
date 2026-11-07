/* Process entry for uClibc 0.9.33: records the initial stack (for getauxval), then calls __uClibc_main. */
.syntax unified
.arch armv5te
.arm
.global _start
.type _start,%function
_start:
    mov r0, sp
    bl qualification_capture_stack
    cmp r0, #0
    beq 1f
    mov r0, #127
    mov r7, #1
    svc #0
1:
    mov fp, #0
    mov lr, #0
    ldr r1, [sp], #4
    mov r2, sp
    mov r6, sp
    bic sp, sp, #7
    mov r0, #0
    sub sp, sp, #16
    str r0, [sp]
    str r0, [sp, #4]
    str r6, [sp, #8]
    ldr r0, =main
    mov r3, #0
    bl __uClibc_main
    b .
.section .note.GNU-stack,"",%progbits
