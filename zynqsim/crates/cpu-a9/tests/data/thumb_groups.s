@@T_SHIFT
	lsls r0, r1, #3
	lsrs r2, r3, #32
	asrs r4, r5, #1
	movs r6, r7
	lsls r1, r2, #31
@@T_ADDSUB
	adds r0, r1, r2
	subs r3, r4, r5
	adds r0, r1, #7
	subs r2, r2, #1
	adds r6, r6, #200
	subs r7, #255
@@T_IMM8
	movs r0, #255
	cmp r0, #10
	adds r1, #5
	subs r1, #6
	movs r2, #0
@@T_DPREG
	ands r0, r1
	eors r2, r3
	lsls r4, r5
	lsrs r6, r7
	asrs r0, r1
	adcs r2, r3
	sbcs r4, r5
	rors r6, r7
	tst r0, r1
	rsbs r2, r3, #0
	cmp r4, r5
	cmn r6, r7
	orrs r0, r1
	muls r2, r3, r2
	bics r4, r5
	mvns r6, r7
@@T_HIREG
	add r8, r1
	add r1, r9
	cmp r8, r1
	mov r12, r0
	mov r0, r12
	mov r9, lr
	add pc, r2
@@T_BX
	bx r3
	blx r4
@@T_LDRLIT
	ldr r0, [pc, #8]
	ldr r7, [pc, #0]
@@T_LDST_REG
	str r0, [r1, r2]
	strh r3, [r4, r5]
	strb r6, [r7, r0]
	ldrsb r1, [r2, r3]
	ldr r4, [r5, r6]
	ldrh r7, [r0, r1]
	ldrb r2, [r3, r4]
	ldrsh r5, [r6, r7]
@@T_LDST_IMM
	str r0, [r1, #4]
	ldr r2, [r3, #124]
	strb r4, [r5, #31]
	ldrb r6, [r7, #1]
	strh r0, [r1, #2]
	ldrh r3, [r4, #62]
	str r5, [sp, #8]
	ldr r6, [sp, #16]
@@T_ADR_SP
	adr r0, here
	add r1, sp, #16
	add sp, #8
	sub sp, #16
here:
	nop
@@T_EXT_REV
	sxth r0, r1
	sxtb r2, r3
	uxth r4, r5
	uxtb r6, r7
	rev r0, r1
	rev16 r2, r3
	revsh r4, r5
@@T_PUSHPOP
	push {r0-r3, lr}
	pop {r0-r3}
	push {r4}
	pop {r4, r5}
@@T_LDMSTM
	stmia r0!, {r1, r2}
	ldmia r3!, {r4, r5}
	ldmia r6, {r6, r7}
	stmia r7!, {r0}
@@T_BR_COND
	beq skip
	bne skip
	bcs skip
	bgt skip
	b skip
	nop
skip:
	nop
@@T_CBZ
	cbz r0, out
	cbnz r1, out
	nop
out:
	nop
@@T_IT
	it eq
	moveq r0, #1
	ite ne
	movne r1, #2
	moveq r1, #3
	itt cs
	addcs r2, r2, #1
	subcs r3, r3, #1
	itete hi
	movhi r4, #4
	movls r4, #5
	movhi r5, #6
	movls r5, #7
@@T_HINTS
	nop
	yield
	wfe
	wfi
	sev
@@T_BKPT_SVC
	bkpt #0x12
	svc #3
@@T_CPS
	cpsid i
	cpsie i
	cpsid f
@@T_UDF
	udf #0x42
@@T_B_NEAR
	b far
	nop
	nop
far:
	nop
@@T32_DPIMM
	add.w r0, r1, #0x100
	and.w r2, r3, #0xff00ff
	orr.w r4, r5, #0x80000000
	mov.w r6, #0x1200
	mvn.w r7, #0
	tst.w r0, #0x80
	teq.w r1, #0x10
	cmp.w r2, #0x1000
	cmn.w r3, #1
	bic.w r4, r4, #0x0f
	orn r5, r5, #0xff
	adc.w r6, r7, #0xf0000000
	sbc.w r8, r9, #0x55555555
	rsb.w r0, r1, #0
	subs.w r2, r3, #0xaa00aa00
@@T32_DPREG
	add.w r0, r1, r2, lsl #3
	sub.w r3, r4, r5, asr #5
	orr.w r6, r7, r8, ror #7
	adds.w r0, r1, r2, rrx
	cmp.w r0, r1, lsr #2
	tst.w r0, r1
	mov.w r0, r1, lsl #4
	mvn.w r2, r3, asr #1
	orn.w r0, r1, r2
	bic.w r0, r1, r2, lsl #2
	adc.w r3, r4, r5, lsr #32
	sbcs.w r6, r7, r8
	rsb.w r0, r1, r2, lsl #1
	eor.w r9, r10, r11
	teq.w r12, lr
	cmn.w r0, r1, asr #3
@@T32_SHIFTREG
	lsl.w r0, r1, r2
	lsrs.w r3, r4, r5
	asr.w r6, r7, r8
	ror.w r0, r1, r2
	lsls.w r9, r10, r11
@@T32_MOVW_ADDW
	movw r0, #0xffff
	movt r0, #0x1234
	addw r1, r2, #0x7ff
	subw r3, r4, #0x123
	movw r5, #0
	movt r5, #0
@@T32_ADR
	adr.w r0, lbl
	adr.w r1, lbl2
	nop
lbl:
	nop
lbl2:
	nop
@@T32_BITFIELD
	ubfx r0, r1, #4, #8
	sbfx r2, r3, #1, #15
	bfi r4, r5, #8, #4
	bfc r6, #0, #5
	clz r0, r1
	rbit r2, r3
	rev.w r4, r5
	rev16.w r6, r7
	revsh.w r8, r9
@@T32_EXTEND
	sxtb.w r0, r1
	uxth.w r2, r3, ror #8
	sxtb.w r4, r5, ror #16
	uxtb.w r6, r7
	sxth.w r8, r9
	sxtab r0, r1, r2
	uxtah r3, r4, r5
@@T32_SAT
	ssat r0, #16, r1
	usat r2, #8, r3, lsl #4
@@T32_MUL
	mul r0, r1, r2
	mla r3, r4, r5, r6
	mls r7, r8, r9, r10
	umull r0, r1, r2, r3
	smull r4, r5, r6, r7
	umlal r0, r1, r2, r3
	smlal r4, r5, r6, r7
	smulbb r0, r1, r2
	smlabt r3, r4, r5, r6
	smultt r7, r8, r9
@@T32_DIV
	.arch_extension idiv
	udiv r0, r1, r2
	sdiv r3, r4, r5
@@T32_LDR
	ldr.w r0, [r1, #0x123]
	str.w r2, [r3, #-8]
	ldr r4, [r5, #-4]!
	ldr r6, [r7], #8
	ldrb.w r0, [r1, #4095]
	strh.w r2, [r3, #1]
	ldrsb.w r4, [r5, #-1]
	ldrsh.w r6, [r7, r8, lsl #1]
	ldr.w r0, [r1, r2, lsl #2]
	str r3, [r4, r5]
	ldrb r1, [r2, #-3]!
	strb r1, [r2], #-1
	ldrh r8, [r9, #-2]
	ldr.w r0, [pc, #-16]
	ldrsh r2, [r3, #0x20]
	ldrd r0, r1, [r2, #8]
	strd r3, r4, [r5, #-16]!
	ldrd r6, r7, [r8], #8
	strd r9, r10, [r11]
	ldrd r0, r1, [pc, #12]
	pld [r0, #4]
	pli [r1, #8]
@@T32_LDM
	stmdb sp!, {r4-r11, lr}
	ldmia.w r0!, {r1-r3}
	ldm.w r4, {r5, r6}
	stmia r7, {r8, r9}
	push.w {r4, r8}
	pop.w {r4, r8, pc}
	ldmdb r1!, {r2, r3}
	stmdb r2, {r3, r4}
@@T32_LDREX
	ldrex r0, [r1]
	strex r2, r3, [r4]
	ldrexb r5, [r6]
	strexh r7, r8, [r9]
	ldrexh r1, [r2]
	strexb r3, r4, [r5]
	ldrexd r0, r1, [r2]
	strexd r3, r4, r5, [r6]
	clrex
@@T32_TBB
	tbb [pc, r0]
	tbh [r1, r2, lsl #1]
@@T32_SYS
	mrs r0, apsr
	mrs r2, cpsr
	msr apsr_nzcvq, r1
	msr cpsr_fc, r3
	mrs r4, spsr
	msr spsr_fsxc, r5
	dmb ish
	dsb sy
	isb sy
	nop.w
	yield.w
	wfe.w
	wfi.w
	sev.w
@@T32_CP15
	mcr p15, 0, r0, c1, c0, 0
	mrc p15, 0, r1, c0, c0, 5
	mrc p15, 0, r2, c0, c0, 0
	mcr p15, 0, r3, c7, c10, 4
@@T32_BRANCH
	b.w lbl
	beq.w lbl
	bl lbl
	blx lbl2
	blx r0
	nop
lbl2:
	nop
lbl:
	nop
