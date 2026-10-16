// Self-checking testbench written for the ZynqSim universality corpus (not upstream code).
// Same hand-assembled RV32IM program as the picorv32 entry, run on uRISC-V with upstream's TCM
// testbench memory (tcm_mem.v / tcm_mem_ram.v, unmodified):
//   x1 = 55 -> ram[0x1000], x5 = x1*x1 = 3025 -> ram[0x1004], x6 = 2*lw(0x1000) = 110 -> ram[0x1008]
`timescale 1ns / 1ps

module corpus_tb;
    reg clk = 0;
    reg rst = 1;
    always #5 clk = ~clk;

    wire          mem_i_rd_w, mem_i_flush_w, mem_i_invalidate_w;
    wire [ 31:0]  mem_i_pc_w;
    wire [ 31:0]  mem_d_addr_w, mem_d_data_wr_w;
    wire          mem_d_rd_w;
    wire [  3:0]  mem_d_wr_w;
    wire          mem_d_cacheable_w, mem_d_invalidate_w, mem_d_writeback_w, mem_d_flush_w;
    wire [ 10:0]  mem_d_req_tag_w, mem_d_resp_tag_w;
    wire          mem_i_accept_w, mem_i_valid_w, mem_i_error_w;
    wire [ 31:0]  mem_i_inst_w, mem_d_data_rd_w;
    wire          mem_d_accept_w, mem_d_ack_w, mem_d_error_w;

    riscv_core u_dut (
        .clk_i(clk), .rst_i(rst),
        .mem_d_data_rd_i(mem_d_data_rd_w), .mem_d_accept_i(mem_d_accept_w),
        .mem_d_ack_i(mem_d_ack_w), .mem_d_error_i(mem_d_error_w),
        .mem_d_resp_tag_i(mem_d_resp_tag_w),
        .mem_i_accept_i(mem_i_accept_w), .mem_i_valid_i(mem_i_valid_w),
        .mem_i_error_i(mem_i_error_w), .mem_i_inst_i(mem_i_inst_w),
        .intr_i(1'b0), .reset_vector_i(32'h0000_0000), .cpu_id_i(32'b0),
        .mem_d_addr_o(mem_d_addr_w), .mem_d_data_wr_o(mem_d_data_wr_w),
        .mem_d_rd_o(mem_d_rd_w), .mem_d_wr_o(mem_d_wr_w),
        .mem_d_cacheable_o(mem_d_cacheable_w), .mem_d_req_tag_o(mem_d_req_tag_w),
        .mem_d_invalidate_o(mem_d_invalidate_w), .mem_d_writeback_o(mem_d_writeback_w),
        .mem_d_flush_o(mem_d_flush_w),
        .mem_i_rd_o(mem_i_rd_w), .mem_i_flush_o(mem_i_flush_w),
        .mem_i_invalidate_o(mem_i_invalidate_w), .mem_i_pc_o(mem_i_pc_w));

    tcm_mem u_mem (
        .clk_i(clk), .rst_i(rst),
        .mem_i_rd_i(mem_i_rd_w), .mem_i_flush_i(mem_i_flush_w),
        .mem_i_invalidate_i(mem_i_invalidate_w), .mem_i_pc_i(mem_i_pc_w),
        .mem_d_addr_i(mem_d_addr_w), .mem_d_data_wr_i(mem_d_data_wr_w),
        .mem_d_rd_i(mem_d_rd_w), .mem_d_wr_i(mem_d_wr_w),
        .mem_d_cacheable_i(mem_d_cacheable_w), .mem_d_req_tag_i(mem_d_req_tag_w),
        .mem_d_invalidate_i(mem_d_invalidate_w), .mem_d_writeback_i(mem_d_writeback_w),
        .mem_d_flush_i(mem_d_flush_w),
        .mem_i_accept_o(mem_i_accept_w), .mem_i_valid_o(mem_i_valid_w),
        .mem_i_error_o(mem_i_error_w), .mem_i_inst_o(mem_i_inst_w),
        .mem_d_data_rd_o(mem_d_data_rd_w), .mem_d_accept_o(mem_d_accept_w),
        .mem_d_ack_o(mem_d_ack_w), .mem_d_error_o(mem_d_error_w),
        .mem_d_resp_tag_o(mem_d_resp_tag_w));

    integer errors = 0;
    integer i;

    task load(input integer index, input [31:0] value);
        begin
            u_mem.write(index * 4 + 0, value[7:0]);
            u_mem.write(index * 4 + 1, value[15:8]);
            u_mem.write(index * 4 + 2, value[23:16]);
            u_mem.write(index * 4 + 3, value[31:24]);
        end
    endtask

    initial begin
        for (i = 0; i < 64; i = i + 1) load(i, 32'h0000_0013);
        load(0,  32'h00000093); // addi x1,x0,0
        load(1,  32'h00100113); // addi x2,x0,1
        load(2,  32'h00b00193); // addi x3,x0,11
        load(3,  32'h002080b3); // loop: add x1,x1,x2
        load(4,  32'h00110113); // addi x2,x2,1
        load(5,  32'hfe311ce3); // bne x2,x3,loop
        load(6,  32'h00001237); // lui x4,1
        load(7,  32'h00122023); // sw x1,0(x4)
        load(8,  32'h021082b3); // mul x5,x1,x1
        load(9,  32'h00522223); // sw x5,4(x4)
        load(10, 32'h00022303); // lw x6,0(x4)
        load(11, 32'h00630333); // add x6,x6,x6
        load(12, 32'h00622423); // sw x6,8(x4)
        load(13, 32'h0000006f); // jal x0,0
        repeat (5) @(posedge clk);
        rst <= 1'b0;
        repeat (3000) @(posedge clk);
        if (u_mem.u_ram.ram[32'h1000 >> 2] !== 32'd55) begin
            $display("ram[0x1000] = %0d, expected 55", u_mem.u_ram.ram[32'h1000 >> 2]);
            errors = errors + 1;
        end
        if (u_mem.u_ram.ram[32'h1004 >> 2] !== 32'd3025) begin
            $display("ram[0x1004] = %0d, expected 3025", u_mem.u_ram.ram[32'h1004 >> 2]);
            errors = errors + 1;
        end
        if (u_mem.u_ram.ram[32'h1008 >> 2] !== 32'd110) begin
            $display("ram[0x1008] = %0d, expected 110", u_mem.u_ram.ram[32'h1008 >> 2]);
            errors = errors + 1;
        end
        if (errors == 0) $display("CORPUS_PASS uriscv");
        else $display("CORPUS_FAIL uriscv (%0d errors)", errors);
        $finish;
    end
endmodule
