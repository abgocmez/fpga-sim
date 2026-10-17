@@ENABLE
mov r12, #0xf00000
mcr p15, 0, r12, c1, c0, 2
mov r12, #0x40000000
vmsr fpexc, r12
@@IN_S2
vmov s2, r5
@@IN_D2
vmov d2, r4, r5
@@OUT_S5
vmov r6, s5
@@OUT_S6
vmov r7, s6
@@OUT_S16
vmov r6, s16
@@OUT_S17
vmov r7, s17
@@OUT_D5
vmov r6, r7, d5
@@ENTER_USER
msr cpsr_c, #16
@@CPACR_PRIV
mov r12, #0x500000
mcr p15, 0, r12, c1, c0, 2
@@SET_FPSCR
vmsr fpscr, r9
@@IN_S0
vmov s0, r0
@@IN_S1
vmov s1, r1
@@IN_D0
vmov d0, r0, r1
@@IN_D1
vmov d1, r2, r3
@@OUT_S2
vmov r6, s2
@@OUT_D2
vmov r6, r7, d2
@@OUT_FPSCR
vmrs r8, fpscr
@@OUT_NZCV
vmrs APSR_nzcv, fpscr
@@OP_ADD_S
vadd.f32 s2, s0, s1
@@OP_SUB_S
vsub.f32 s2, s0, s1
@@OP_MUL_S
vmul.f32 s2, s0, s1
@@OP_DIV_S
vdiv.f32 s2, s0, s1
@@OP_SQRT_S
vsqrt.f32 s2, s0
@@OP_MLA_S
vmla.f32 s2, s0, s1
@@OP_MLS_S
vmls.f32 s2, s0, s1
@@OP_NMLA_S
vnmla.f32 s2, s0, s1
@@OP_NMLS_S
vnmls.f32 s2, s0, s1
@@OP_NMUL_S
vnmul.f32 s2, s0, s1
@@OP_ADD_D
vadd.f64 d2, d0, d1
@@OP_SUB_D
vsub.f64 d2, d0, d1
@@OP_MUL_D
vmul.f64 d2, d0, d1
@@OP_DIV_D
vdiv.f64 d2, d0, d1
@@OP_SQRT_D
vsqrt.f64 d2, d0
@@OP_MLA_D
vmla.f64 d2, d0, d1
@@OP_MLS_D
vmls.f64 d2, d0, d1
@@OP_NMLA_D
vnmla.f64 d2, d0, d1
@@OP_NMLS_D
vnmls.f64 d2, d0, d1
@@OP_NMUL_D
vnmul.f64 d2, d0, d1
@@OP_ABS_S
vabs.f32 s2, s0
@@OP_NEG_S
vneg.f32 s2, s0
@@OP_MOV_S
vmov.f32 s2, s0
@@OP_ABS_D
vabs.f64 d2, d0
@@OP_NEG_D
vneg.f64 d2, d0
@@OP_MOV_D
vmov.f64 d2, d0
@@OP_IMM_S
vmov.f32 s2, #1.0
@@OP_IMM_D
vmov.f64 d2, #-2.5
@@OP_CVT_S2D
vcvt.f64.f32 d2, s0
@@OP_CVT_D2S
vcvt.f32.f64 s2, d0
@@OP_I2F_S
vcvt.f32.s32 s2, s0
@@OP_U2F_S
vcvt.f32.u32 s2, s0
@@OP_I2F_D
vcvt.f64.s32 d2, s0
@@OP_U2F_D
vcvt.f64.u32 d2, s0
@@OP_F2I_RZ_S
vcvt.s32.f32 s2, s0
@@OP_F2I_R_S
vcvtr.s32.f32 s2, s0
@@OP_F2U_RZ_S
vcvt.u32.f32 s2, s0
@@OP_F2I_RZ_D
vcvt.s32.f64 s2, d0
@@OP_F2I_R_D
vcvtr.s32.f64 s2, d0
@@OP_F2U_R_S
vcvtr.u32.f32 s2, s0
@@OP_F2U_R_D
vcvtr.u32.f64 s2, d0
@@OP_FIX_F2S32
vcvt.s32.f32 s2, s2, #16
@@OP_FIX_F2S5
vcvt.s32.f32 s2, s2, #5
@@OP_FIX_F2S16
vcvt.s16.f32 s2, s2, #5
@@OP_FIX_I2F_S32
vcvt.f32.s32 s2, s2, #16
@@OP_FIX_I2F_U16
vcvt.f32.u16 s2, s2, #8
@@OP_FIX_F2D_S32
vcvt.s32.f64 d2, d2, #5
@@OP_CMP_S
vcmp.f32 s0, s1
@@OP_CMPE_S
vcmpe.f32 s0, s1
@@OP_CMP_D
vcmp.f64 d0, d1
@@OP_CMPE_D
vcmpe.f64 d0, d1
@@OP_CMPZ_S
vcmp.f32 s0, #0.0
@@OP_CMPEZ_D
vcmpe.f64 d0, #0.0
@@OP_VFMA_S
vfma.f32 s2, s0, s1
@@OP_VFNMA_D
vfnma.f64 d2, d0, d1
@@OP_VRINTZ_S
vrintz.f32 s2, s0
@@OP_VCVTB
vcvtb.f32.f16 s2, s0
@@XFER_S5_R2
vmov s5, r2
@@XFER_R2_S5
vmov r2, s5
@@XFER_S56_R23
vmov s5, s6, r2, r3
@@XFER_R23_S56
vmov r2, r3, s5, s6
@@XFER_D5_R23
vmov d5, r2, r3
@@XFER_R23_D5
vmov r2, r3, d5
@@VMRS_FPSID
vmrs r0, fpsid
@@VMRS_MVFR0
vmrs r0, mvfr0
@@VMRS_FPEXC
vmrs r0, fpexc
@@VMSR_FPEXC
vmsr fpexc, r4
@@MEM_VLDR_S
vldr s2, [r4, #8]
@@MEM_VLDR_D
vldr d2, [r4, #16]
@@MEM_VSTR_D
vstr d3, [r4, #24]
@@MEM_VSTR_S
vstr s3, [r4, #4]
@@MEM_VLDMIA_D
vldmia r4!, {d4-d5}
@@MEM_VSTMDB_S
vstmdb r4!, {s4-s5}
@@MEM_VPUSH_D
vpush {d8-d9}
@@MEM_VPOP_S
vpop {s16-s17}
@@MEM_VSTMIA_S
vstmia r4, {s6-s7}
@@OP_VNMUL_NOOP
vmov.f32 s0, s0
