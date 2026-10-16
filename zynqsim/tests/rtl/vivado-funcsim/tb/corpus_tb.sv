// Self-checking testbench written for the ZynqSim universality corpus.
// Drives the Vivado 2022.1 post-synthesis functional netlist of `my_math_ip`
// (`write_verilog -mode funcsim`, UNISIM LUT/FDRE cells, built with `-L unisims_ver`) over its
// AXI4-Lite slave port: register 0 echoes the written value, register 1 reads back twice that.
`timescale 1ns / 1ps

module corpus_tb;
    reg aclk = 0;
    reg aresetn = 0;
    always #5 aclk = ~aclk;

    reg  [3:0]  awaddr = 0, araddr = 0;
    reg         awvalid = 0, wvalid = 0, bready = 0, arvalid = 0, rready = 0;
    reg  [31:0] wdata = 0;
    wire        awready, wready, bvalid, arready, rvalid;
    wire [31:0] rdata;
    wire [1:0]  bresp, rresp;

    my_math_ip dut (
        .S_AXI_ACLK(aclk), .S_AXI_ARESETN(aresetn),
        .S_AXI_AWADDR(awaddr), .S_AXI_AWPROT(3'b000), .S_AXI_AWVALID(awvalid),
        .S_AXI_AWREADY(awready),
        .S_AXI_WDATA(wdata), .S_AXI_WSTRB(4'hF), .S_AXI_WVALID(wvalid), .S_AXI_WREADY(wready),
        .S_AXI_BRESP(bresp), .S_AXI_BVALID(bvalid), .S_AXI_BREADY(bready),
        .S_AXI_ARADDR(araddr), .S_AXI_ARPROT(3'b000), .S_AXI_ARVALID(arvalid),
        .S_AXI_ARREADY(arready),
        .S_AXI_RDATA(rdata), .S_AXI_RRESP(rresp), .S_AXI_RVALID(rvalid),
        .S_AXI_RREADY(rready));

    integer errors = 0;

    task axi_write(input [3:0] a, input [31:0] d);
        begin
            @(posedge aclk);
            awaddr <= a; awvalid <= 1'b1; wdata <= d; wvalid <= 1'b1;
            @(posedge aclk);
            while (!(awready && wready)) @(posedge aclk);
            awvalid <= 1'b0; wvalid <= 1'b0; bready <= 1'b1;
            @(posedge aclk);
            while (!bvalid) @(posedge aclk);
            bready <= 1'b0;
        end
    endtask

    task axi_read(input [3:0] a, output [31:0] d);
        begin
            @(posedge aclk);
            araddr <= a; arvalid <= 1'b1;
            @(posedge aclk);
            while (!arready) @(posedge aclk);
            arvalid <= 1'b0; rready <= 1'b1;
            @(posedge aclk);
            while (!rvalid) @(posedge aclk);
            d = rdata;
            rready <= 1'b0;
        end
    endtask

    reg [31:0] got;
    reg [31:0] vectors [0:5];
    integer i;

    initial begin
        vectors[0] = 32'd0;          vectors[1] = 32'd1;
        vectors[2] = 32'd1000;       vectors[3] = 32'h1234_5678;
        vectors[4] = 32'h7FFF_FFFF;  vectors[5] = 32'hDEAD_BEEF;
        // `glbl` holds the global set/reset for 100 ns; the netlist's flip-flops ignore the clock
        // until it is released.
        #200;
        repeat (5) @(posedge aclk);
        aresetn <= 1'b1;
        repeat (3) @(posedge aclk);
        for (i = 0; i < 6; i = i + 1) begin
            axi_write(4'h0, vectors[i]);
            axi_read(4'h0, got);
            if (got !== vectors[i]) begin
                $display("reg0 after writing %08x: %08x", vectors[i], got);
                errors = errors + 1;
            end
            axi_read(4'h4, got);
            if (got !== (vectors[i] << 1)) begin
                $display("reg1 after writing %08x: %08x, expected %08x", vectors[i], got,
                         vectors[i] << 1);
                errors = errors + 1;
            end
        end
        if (errors == 0) $display("CORPUS_PASS vivado-funcsim");
        else $display("CORPUS_FAIL vivado-funcsim (%0d errors)", errors);
        $finish;
    end

    initial begin
        #100000;
        $display("CORPUS_FAIL vivado-funcsim (timeout)");
        $finish;
    end
endmodule
