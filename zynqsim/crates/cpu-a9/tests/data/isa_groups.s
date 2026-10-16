@@PROG_BRANCH
	mov r0, #0
	bl sub1
	add r0, r0, #1
	b end
sub1:
	add r0, r0, #10
	bx lr
end:
	mov r1, #7
@@PROG_LDST
	mov r1, #0x2000
	mov r2, #0x11
	mov r3, #0x22
	str r2, [r1]
	strb r3, [r1, #4]
	ldr r4, [r1]
	ldrb r5, [r1, #4]
	str r2, [r1, #8]!
	ldr r6, [r1], #-8
	mov r7, #0x80
	strh r7, [r1, #0x10]
	ldrsh r8, [r1, #0x10]
	mov r9, #0xFF
	strb r9, [r1, #0x12]
	ldrsb r10, [r1, #0x12]
	ldrh r11, [r1, #0x10]
	strd r4, r5, [r1, #0x20]
	ldrd r6, r7, [r1, #0x20]
@@PROG_LDM
	mov r1, #0x2000
	mov r4, #4
	mov r5, #5
	mov r6, #6
	stmia r1!, {r4-r6}
	ldmdb r1!, {r7-r9}
	stmib r1, {r4, r5}
	add r1, r1, #8
	ldmda r1!, {r10, r11}
@@PROG_MUL
	mov r1, #7
	mov r2, #6
	mul r0, r1, r2
	mla r3, r1, r2, r0
	mls r4, r1, r2, r3
	mvn r5, #0
	umull r6, r7, r5, r5
	smull r8, r9, r5, r5
	umlal r6, r7, r1, r2
	smlal r8, r9, r5, r2
	muls r10, r5, r5
@@PROG_HALFMUL
	movw r1, #0xFFFF
	movw r2, #2
	movt r2, #2
	smulbb r0, r1, r2
	smlabt r3, r1, r2, r2
@@PROG_BITS
	movw r1, #0x3412
	movt r1, #0x7856
	rev r2, r1
	rev16 r3, r1
	revsh r4, r1
	rbit r5, r1
	clz r6, r1
	mov r7, #0
	clz r8, r7
	ubfx r9, r1, #8, #8
	sbfx r10, r1, #28, #4
	mvn r12, #0
	bfi r12, r1, #4, #8
	bfc r12, #0, #4
@@PROG_EXTEND
	movw r1, #0xFF80
	sxtb r2, r1
	uxtb r3, r1
	sxth r4, r1
	uxth r5, r1
	mov r7, #1
	uxtab r6, r7, r1
	sxtah r8, r7, r1
@@PROG_SAT
	mov r4, #0x12c
	ssat r3, #8, r4
	mvn r6, #4
	usat r5, #8, r6
	movw r8, #0xFFFF
	movt r8, #0x7FFF
	mov r9, #1
	qadd r7, r8, r9
@@PROG_PAR
	movw r1, #0x0102
	movt r1, #0x0304
	movw r2, #0x0101
	movt r2, #0x0101
	uqsub8 r3, r1, r2
	sadd8 r4, r1, r2
	sadd16 r5, r1, r2
	uhadd16 r6, r1, r2
@@PROG_PKH
	movw r1, #0x5678
	movt r1, #0x1234
	movw r2, #0xABCD
	movt r2, #0xEF01
	pkhbt r3, r1, r2, lsl #4
	pkhtb r4, r1, r2, asr #4
@@PROG_STATUS
	mov r0, #0xF0000000
	msr cpsr_f, r0
	mrs r1, cpsr
	msr cpsr_c, #0x10
	mrs r2, cpsr
@@PROG_BANK
	mov sp, #0x2000
	msr cpsr_c, #0x12
	mov sp, #0x3000
	msr cpsr_c, #0x13
@@PROG_SVC_TRAP
	movw r0, #0x2000
	mcr p15, 0, r0, c12, c0, 0
	msr cpsr_c, #0x10
	svc #0x42
@@PROG_UNDEF
	.word 0xE7F000F0
@@PROG_CP15
	mrc p15, 0, r0, c0, c0, 0
	mrc p15, 0, r1, c0, c0, 5
	movw r2, #0x1234
	mcr p15, 0, r2, c1, c0, 1
	mrc p15, 0, r3, c1, c0, 1
	mcr p15, 0, r2, c7, c5, 0
@@PROG_EXCL
	mov r2, #0x2000
	ldrex r1, [r2]
	mov r4, #9
	strex r3, r4, [r2]
	strex r5, r4, [r2]
	ldrex r6, [r2]
	clrex
	strex r7, r4, [r2]
@@PROG_WFE
	sev
	wfe
	wfe
@@PROG_VFP
	vmov s0, r0
@@PROG_BXODD
	movw r1, #0x2001
	bx r1
@@PROG_LDRPC
	mov r3, #0x1000
	add r3, r3, #0x20
	str r3, [sp, #-4]!
	ldr pc, [sp], #4
@@PROG_MSR_SPSR
	msr spsr_cxsf, #0x13
	mrs r0, spsr
@@PROG_CMPS
	mov r0, #5
	cmp r0, #5
	moveq r1, #1
	movne r1, #2
	tst r0, #4
	teq r0, #5
	cmn r0, #1
@@PROG_DABORT
	movw r1, #0
	movt r1, #0x9000
	ldr r2, [r1]
@@PROG_PABORT
	movw r1, #0
	movt r1, #0x9000
	bx r1
@@PROG_IRQ
	movw r0, #0x2000
	mcr p15, 0, r0, c12, c0, 0
	msr cpsr_c, #0x10
	mov r0, #1
	mov r0, #2
@@PROG_WFI
	wfi
	mov r0, #9
@@PROG_MMU
	mov r0, #1
	mcr p15, 0, r0, c1, c0, 0
	mov r1, #2
