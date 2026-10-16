// Self-checking testbench written for the ZynqSim universality corpus (not upstream code).
// Feeds the ASCII string "123456789" through two lfsr_crc configurations and compares with the
// published check values: CRC-32 (Ethernet) 0xCBF43926 and CRC-16/XMODEM 0x31C3.
`timescale 1ns / 1ps

module corpus_tb;
    reg clk = 0;
    reg rst = 1;
    reg [7:0] data = 8'h00;
    reg valid = 1'b0;
    wire [31:0] crc32;
    wire [15:0] crc16;

    lfsr_crc #(
        .LFSR_WIDTH(32), .LFSR_POLY(32'h04c11db7), .LFSR_INIT(32'hffffffff),
        .LFSR_CONFIG("GALOIS"), .REVERSE(1), .INVERT(1), .DATA_WIDTH(8), .STYLE("AUTO")
    ) u_crc32 (.clk(clk), .rst(rst), .data_in(data), .data_in_valid(valid), .crc_out(crc32));

    lfsr_crc #(
        .LFSR_WIDTH(16), .LFSR_POLY(16'h1021), .LFSR_INIT(16'h0000),
        .LFSR_CONFIG("GALOIS"), .REVERSE(0), .INVERT(0), .DATA_WIDTH(8), .STYLE("AUTO")
    ) u_crc16 (.clk(clk), .rst(rst), .data_in(data), .data_in_valid(valid), .crc_out(crc16));

    always #5 clk = ~clk;

    reg [71:0] msg = "123456789";
    integer i;
    integer errors = 0;

    initial begin
        repeat (3) @(posedge clk);
        rst <= 1'b0;
        @(posedge clk);
        for (i = 8; i >= 0; i = i - 1) begin
            data  <= msg[i*8 +: 8];
            valid <= 1'b1;
            @(posedge clk);
        end
        valid <= 1'b0;
        @(posedge clk);
        #1;
        if (crc32 !== 32'hCBF43926) begin
            $display("crc32 = %08x, expected cbf43926", crc32);
            errors = errors + 1;
        end
        if (crc16 !== 16'h31C3) begin
            $display("crc16 = %04x, expected 31c3", crc16);
            errors = errors + 1;
        end
        if (errors == 0) $display("CORPUS_PASS forencich-lfsr");
        else $display("CORPUS_FAIL forencich-lfsr (%0d errors)", errors);
        $finish;
    end
endmodule
