// Self-checking testbench written for the ZynqSim universality corpus (not upstream code).
// Runs a hand-assembled RV32I program on SERV (the bit-serial RISC-V core) behind a Wishbone
// memory that acknowledges one cycle after a request:
//   x1 = 1+2+...+10 = 55        -> mem[0x1000]
//   x5 = x1 - x2    = 55 - 11   -> mem[0x1004] (= 44)
//   x6 = lw(0x1000) * 2 = 110   -> mem[0x1008]
// then spins in `jal x0, 0`. SERV needs dozens of cycles per instruction, so the run is long.
`timescale 1ns / 1ps

module corpus_tb;
    reg clk = 0;
    reg rst = 1;
    always #5 clk = ~clk;

    wire [31:0] ibus_adr, dbus_adr, dbus_dat;
    wire        ibus_cyc, dbus_cyc, dbus_we;
    wire [3:0]  dbus_sel;
    reg  [31:0] ibus_rdt = 0, dbus_rdt = 0;
    reg         ibus_ack = 0, dbus_ack = 0;

    serv_rf_top #(.RESET_PC(32'h0)) dut (
        .clk(clk), .i_rst(rst), .i_timer_irq(1'b0),
        .o_ibus_adr(ibus_adr), .o_ibus_cyc(ibus_cyc), .i_ibus_rdt(ibus_rdt),
        .i_ibus_ack(ibus_ack),
        .o_dbus_adr(dbus_adr), .o_dbus_dat(dbus_dat), .o_dbus_sel(dbus_sel),
        .o_dbus_we(dbus_we), .o_dbus_cyc(dbus_cyc), .i_dbus_rdt(dbus_rdt),
        .i_dbus_ack(dbus_ack),
        .o_ext_rs1(), .o_ext_rs2(), .o_ext_funct3(), .i_ext_rd(32'b0), .i_ext_ready(1'b0),
        .o_mdu_valid());

    reg [31:0] mem [0:2047];
    integer i;
    integer errors = 0;
    integer cycles = 0;

    initial begin
        for (i = 0; i < 2048; i = i + 1) mem[i] = 32'h0000_0013; // nop
        mem[0]  = 32'h00000093; // addi x1,x0,0
        mem[1]  = 32'h00100113; // addi x2,x0,1
        mem[2]  = 32'h00b00193; // addi x3,x0,11
        mem[3]  = 32'h002080b3; // loop: add x1,x1,x2
        mem[4]  = 32'h00110113; // addi x2,x2,1
        mem[5]  = 32'hfe311ce3; // bne x2,x3,loop
        mem[6]  = 32'h00001237; // lui x4,1
        mem[7]  = 32'h00122023; // sw x1,0(x4)
        mem[8]  = 32'h402082b3; // sub x5,x1,x2
        mem[9]  = 32'h00522223; // sw x5,4(x4)
        mem[10] = 32'h00022303; // lw x6,0(x4)
        mem[11] = 32'h00630333; // add x6,x6,x6
        mem[12] = 32'h00622423; // sw x6,8(x4)
        mem[13] = 32'h0000006f; // jal x0,0
    end

    always @(posedge clk) begin
        ibus_ack <= ibus_cyc & ~ibus_ack;
        ibus_rdt <= mem[ibus_adr[12:2]];
        dbus_ack <= dbus_cyc & ~dbus_ack;
        dbus_rdt <= mem[dbus_adr[12:2]];
        if (dbus_cyc && dbus_we && !dbus_ack) begin
            if (dbus_sel[0]) mem[dbus_adr[12:2]][7:0]   <= dbus_dat[7:0];
            if (dbus_sel[1]) mem[dbus_adr[12:2]][15:8]  <= dbus_dat[15:8];
            if (dbus_sel[2]) mem[dbus_adr[12:2]][23:16] <= dbus_dat[23:16];
            if (dbus_sel[3]) mem[dbus_adr[12:2]][31:24] <= dbus_dat[31:24];
        end
        cycles <= cycles + 1;
    end

    initial begin
        repeat (8) @(posedge clk);
        rst <= 1'b0;
        // Done when the last store has landed; give up after 100k cycles.
        while (mem[32'h1008 >> 2] !== 32'd110 && cycles < 100000) @(posedge clk);
        repeat (50) @(posedge clk);
        if (mem[32'h1000 >> 2] !== 32'd55)  begin $display("mem[0x1000] = %0d, expected 55", mem[32'h1000 >> 2]); errors = errors + 1; end
        if (mem[32'h1004 >> 2] !== 32'd44)  begin $display("mem[0x1004] = %0d, expected 44", mem[32'h1004 >> 2]); errors = errors + 1; end
        if (mem[32'h1008 >> 2] !== 32'd110) begin $display("mem[0x1008] = %0d, expected 110", mem[32'h1008 >> 2]); errors = errors + 1; end
        if (errors == 0) $display("CORPUS_PASS serv");
        else $display("CORPUS_FAIL serv (%0d errors, %0d cycles)", errors, cycles);
        $finish;
    end
endmodule
