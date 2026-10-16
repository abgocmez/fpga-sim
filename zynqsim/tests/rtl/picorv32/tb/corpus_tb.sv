// Self-checking testbench written for the ZynqSim universality corpus (not upstream code).
// Runs a hand-assembled RV32IM program on picorv32 (native memory interface, one-cycle memory):
//   x1 = 1+2+...+10 = 55          -> mem[0x1000]
//   x5 = x1 * x1    = 3025        -> mem[0x1004]   (picorv32_pcpi_mul)
//   x6 = lw(0x1000) * 2 = 110     -> mem[0x1008]
// then spins in `jal x0, 0`. The testbench checks the three stores.
`timescale 1ns / 1ps

module corpus_tb;
    reg clk = 0;
    reg resetn = 0;
    always #5 clk = ~clk;

    wire        trap;
    wire        mem_valid, mem_instr;
    reg         mem_ready = 1'b0;
    wire [31:0] mem_addr, mem_wdata;
    wire [3:0]  mem_wstrb;
    reg  [31:0] mem_rdata;

    picorv32 #(.ENABLE_MUL(1), .ENABLE_DIV(0), .ENABLE_COUNTERS(0), .PROGADDR_RESET(32'h0))
    u_cpu (
        .clk(clk), .resetn(resetn), .trap(trap),
        .mem_valid(mem_valid), .mem_instr(mem_instr), .mem_ready(mem_ready),
        .mem_addr(mem_addr), .mem_wdata(mem_wdata), .mem_wstrb(mem_wstrb),
        .mem_rdata(mem_rdata),
        .pcpi_wr(1'b0), .pcpi_rd(32'b0), .pcpi_wait(1'b0), .pcpi_ready(1'b0),
        .irq(32'b0));

    reg [31:0] mem [0:2047];
    integer i;
    integer errors = 0;

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
        mem[8]  = 32'h021082b3; // mul x5,x1,x1
        mem[9]  = 32'h00522223; // sw x5,4(x4)
        mem[10] = 32'h00022303; // lw x6,0(x4)
        mem[11] = 32'h00630333; // add x6,x6,x6
        mem[12] = 32'h00622423; // sw x6,8(x4)
        mem[13] = 32'h0000006f; // jal x0,0
    end

    // One-cycle memory.
    always @(posedge clk) begin
        mem_ready <= 1'b0;
        if (resetn && mem_valid && !mem_ready) begin
            mem_ready <= 1'b1;
            mem_rdata <= mem[mem_addr[12:2]];
            if (mem_wstrb[0]) mem[mem_addr[12:2]][7:0]   <= mem_wdata[7:0];
            if (mem_wstrb[1]) mem[mem_addr[12:2]][15:8]  <= mem_wdata[15:8];
            if (mem_wstrb[2]) mem[mem_addr[12:2]][23:16] <= mem_wdata[23:16];
            if (mem_wstrb[3]) mem[mem_addr[12:2]][31:24] <= mem_wdata[31:24];
        end
    end

    initial begin
        repeat (8) @(posedge clk);
        resetn <= 1'b1;
        repeat (3000) @(posedge clk);
        if (trap) begin
            $display("the CPU trapped");
            errors = errors + 1;
        end
        if (mem[32'h1000 >> 2] !== 32'd55)   begin $display("mem[0x1000] = %0d, expected 55", mem[32'h1000 >> 2]); errors = errors + 1; end
        if (mem[32'h1004 >> 2] !== 32'd3025) begin $display("mem[0x1004] = %0d, expected 3025", mem[32'h1004 >> 2]); errors = errors + 1; end
        if (mem[32'h1008 >> 2] !== 32'd110)  begin $display("mem[0x1008] = %0d, expected 110", mem[32'h1008 >> 2]); errors = errors + 1; end
        if (errors == 0) $display("CORPUS_PASS picorv32");
        else $display("CORPUS_FAIL picorv32 (%0d errors)", errors);
        $finish;
    end
endmodule
