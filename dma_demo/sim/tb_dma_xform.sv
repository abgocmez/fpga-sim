`timescale 1ns / 1ps
// Stand-alone self-checking testbench for dma_xform (no QEMU): behavioural AXI4
// memory with random stalls, AXI4-Lite master tasks, interrupt check.
module tb_dma_xform;
    reg aclk = 0, aresetn = 0;
    always #5 aclk = ~aclk;

    // AXI-Lite
    reg  [5:0] awaddr = 0, araddr = 0;
    reg        awvalid = 0, wvalid = 0, bready = 0, arvalid = 0, rready = 0;
    reg [31:0] wdata = 0;
    wire       awready, wready, bvalid, arready, rvalid;
    wire [31:0] rdata;
    wire [1:0] bresp, rresp;
    // AXI4 master
    wire [31:0] m_araddr, m_awaddr, m_wdata, m_rdata;
    wire [7:0]  m_arlen, m_awlen;
    wire [2:0]  m_arsize, m_awsize;
    wire [1:0]  m_arburst, m_awburst;
    wire        m_arvalid, m_awvalid, m_wvalid, m_wlast, m_rready, m_bready;
    wire [3:0]  m_wstrb;
    reg         m_arready = 0, m_awready = 0, m_wready = 0, m_rvalid = 0, m_rlast = 0, m_bvalid = 0;
    reg [31:0]  m_rdata_r = 0;
    reg [1:0]   m_bresp = 0;
    assign m_rdata = m_rdata_r;
    wire irq;

    dma_xform dut (
        .aclk(aclk), .aresetn(aresetn),
        .s_awaddr(awaddr), .s_awvalid(awvalid), .s_awready(awready),
        .s_wdata(wdata), .s_wstrb(4'hF), .s_wvalid(wvalid), .s_wready(wready),
        .s_bresp(bresp), .s_bvalid(bvalid), .s_bready(bready),
        .s_araddr(araddr), .s_arvalid(arvalid), .s_arready(arready),
        .s_rdata(rdata), .s_rresp(rresp), .s_rvalid(rvalid), .s_rready(rready),
        .m_araddr(m_araddr), .m_arlen(m_arlen), .m_arsize(m_arsize), .m_arburst(m_arburst),
        .m_arvalid(m_arvalid), .m_arready(m_arready),
        .m_rdata(m_rdata), .m_rresp(2'b00), .m_rlast(m_rlast), .m_rvalid(m_rvalid), .m_rready(m_rready),
        .m_awaddr(m_awaddr), .m_awlen(m_awlen), .m_awsize(m_awsize), .m_awburst(m_awburst),
        .m_awvalid(m_awvalid), .m_awready(m_awready),
        .m_wdata(m_wdata), .m_wstrb(m_wstrb), .m_wlast(m_wlast), .m_wvalid(m_wvalid), .m_wready(m_wready),
        .m_bresp(m_bresp), .m_bvalid(m_bvalid), .m_bready(m_bready),
        .irq(irq)
    );

    // ------------------------------------------------ behavioural AXI4 memory
    reg [31:0] mem [0:16383];            // 64 KB
    integer errors = 0;
    integer stall_en = 1;
    integer bursts_r = 0, bursts_w = 0, max_len = 0, cross4k = 0;

    function integer stall; input dummy; begin stall = stall_en ? ($urandom % 3) : 0; end endfunction

    // read channel
    reg [31:0] ra; reg [7:0] rl; integer ri;
    initial forever begin
        @(posedge aclk);
        if (m_arvalid && !m_arready) begin
            repeat (stall(0)) @(posedge aclk);
            m_arready <= 1'b1;
            @(posedge aclk);
            m_arready <= 1'b0;
            ra = m_araddr; rl = m_arlen; bursts_r = bursts_r + 1;
            if (rl + 1 > max_len) max_len = rl + 1;
            if ((ra[11:0] >> 2) + rl + 1 > 1024) cross4k = cross4k + 1;
            for (ri = 0; ri <= rl; ri = ri + 1) begin
                repeat (stall(0)) @(posedge aclk);
                m_rdata_r <= mem[(ra >> 2) + ri];
                m_rvalid  <= 1'b1;
                m_rlast   <= (ri == rl);
                @(posedge aclk);
                while (!m_rready) @(posedge aclk);
                m_rvalid <= 1'b0; m_rlast <= 1'b0;
            end
        end
    end

    // write channel
    reg [31:0] wa; reg [7:0] wl; integer wi;
    initial forever begin
        @(posedge aclk);
        if (m_awvalid && !m_awready) begin
            repeat (stall(0)) @(posedge aclk);
            m_awready <= 1'b1;
            @(posedge aclk);
            m_awready <= 1'b0;
            wa = m_awaddr; wl = m_awlen; bursts_w = bursts_w + 1;
            if ((wa[11:0] >> 2) + wl + 1 > 1024) cross4k = cross4k + 1;
            for (wi = 0; wi <= wl; wi = wi + 1) begin
                repeat (stall(0)) @(posedge aclk);
                m_wready <= 1'b1;
                @(posedge aclk);
                while (!m_wvalid) @(posedge aclk);
                mem[(wa >> 2) + wi] = m_wdata;
                if (m_wlast !== (wi == wl)) begin
                    errors = errors + 1; $display("ERR wlast mismatch at beat %0d", wi);
                end
                m_wready <= 1'b0;
            end
            repeat (stall(0)) @(posedge aclk);
            m_bvalid <= 1'b1; m_bresp <= 2'b00;
            @(posedge aclk);
            while (!m_bready) @(posedge aclk);
            m_bvalid <= 1'b0;
        end
    end

    // ------------------------------------------------------ AXI-Lite tasks
    task lite_wr(input [5:0] a, input [31:0] d);
        begin
            @(posedge aclk);
            awaddr <= a; wdata <= d; awvalid <= 1; wvalid <= 1;
            @(posedge aclk);
            while (!(awready && wready)) @(posedge aclk);
            awvalid <= 0; wvalid <= 0; bready <= 1;
            @(posedge aclk);
            while (!bvalid) @(posedge aclk);
            bready <= 0;
        end
    endtask
    task lite_rd(input [5:0] a, output [31:0] d);
        begin
            @(posedge aclk);
            araddr <= a; arvalid <= 1;
            @(posedge aclk);
            while (!arready) @(posedge aclk);
            arvalid <= 0; rready <= 1;
            @(posedge aclk);
            while (!rvalid) @(posedge aclk);
            d = rdata; rready <= 0;
        end
    endtask

    // ------------------------------------------------------------ test body
    reg [31:0] v;
    integer i, t;

    reg sat_mode = 0;
    task run_case(input [31:0] src, input [31:0] dst, input [31:0] len,
                  input [31:0] scale, input [31:0] off, input use_irq, input expect_err);
        reg [31:0] exp_sum, w;
        integer k, guard;
        begin
            for (k = 0; k < len; k = k + 1) mem[(src >> 2) + k] = sat_mode ? ($urandom % 300) : $urandom;
            for (k = 0; k < len + 4; k = k + 1) mem[(dst >> 2) + k] = 32'hDEAD_BEEF;
            lite_wr(6'h08, src); lite_wr(6'h0C, dst); lite_wr(6'h10, len);
            lite_wr(6'h14, scale); lite_wr(6'h18, off);
            lite_wr(6'h00, {29'd0, sat_mode, use_irq, 1'b1});   // SAT, IRQ_EN, START
            guard = 0;
            if (use_irq) begin
                while (!irq && guard < 200000) begin @(posedge aclk); guard = guard + 1; end
                if (!irq) begin errors = errors + 1; $display("ERR irq never rose"); end
            end else begin
                v = 0;
                while (!v[1] && guard < 200000) begin lite_rd(6'h04, v); guard = guard + 1; end
            end
            lite_rd(6'h04, v);
            if (expect_err) begin
                if (!v[2] || !v[1]) begin errors = errors + 1; $display("ERR expected error status, got %h", v); end
            end else begin
                exp_sum = 0;
                for (k = 0; k < len; k = k + 1) begin
                    w = mem[(src >> 2) + k] * scale + off;
                    if (sat_mode) w = w[31] ? 32'd0 : (w > 255) ? 32'd255 : w;
                    exp_sum = exp_sum + w;
                    if (mem[(dst >> 2) + k] !== w) begin
                        errors = errors + 1;
                        if (errors < 10) $display("ERR dst[%0d]=%h exp %h", k, mem[(dst >> 2) + k], w);
                    end
                end
                if (mem[(dst >> 2) + len] !== 32'hDEAD_BEEF) begin
                    errors = errors + 1; $display("ERR write past end of destination");
                end
                lite_rd(6'h1C, v);
                if (v !== exp_sum) begin errors = errors + 1; $display("ERR checksum %h exp %h", v, exp_sum); end
                lite_rd(6'h20, v);
                if (v !== len) begin errors = errors + 1; $display("ERR words %0d exp %0d", v, len); end
            end
            lite_wr(6'h04, 32'h6);                          // clear DONE, ERROR
            repeat (3) @(posedge aclk);
            if (irq) begin errors = errors + 1; $display("ERR irq still asserted after clear"); end
            $display("case src=%h dst=%h len=%0d scale=%0d off=%0d irq=%0d : done", src, dst, len, scale, off, use_irq);
        end
    endtask

    initial begin
        repeat (10) @(posedge aclk);
        aresetn = 1;
        repeat (5) @(posedge aclk);

        run_case(32'h0000_0000, 32'h0000_4000, 16,   3, 7,   1, 0);   // exactly one burst
        run_case(32'h0000_0100, 32'h0000_5000, 100,  5, 1,   0, 0);   // 6 full bursts + remainder
        run_case(32'h0000_0F80, 32'h0000_6F00, 200,  2, 0,   1, 0);   // crosses 4 KB in src and dst
        run_case(32'h0000_8000, 32'h0000_9000, 1,    9, 9,   1, 0);   // single word
        run_case(32'h0000_0002, 32'h0000_9000, 8,    1, 0,   0, 1);   // unaligned src -> error
        run_case(32'h0000_0000, 32'h0000_9000, 0,    1, 0,   0, 1);   // len 0 -> error
        sat_mode = 1;                                                   // clamp to 0..255
        run_case(32'h0000_C000, 32'h0000_D000, 300,  2, 32'hFFFF_FF9C, 1, 0);   // x*2-100, saturating
        run_case(32'h0000_C000, 32'h0000_D000, 300,  1, 40,  0, 0);
        sat_mode = 0;
        stall_en = 0;
        run_case(32'h0000_A000, 32'h0000_B000, 1000, 1, 100, 1, 0);   // back-to-back, no stalls

        $display("bursts read=%0d write=%0d max_len=%0d crossings_of_4K=%0d", bursts_r, bursts_w, max_len, cross4k);
        if (cross4k != 0) begin errors = errors + 1; $display("ERR burst crossed 4 KB boundary"); end
        if (errors == 0) $display("TB PASS"); else $display("TB FAIL: %0d errors", errors);
        $finish;
    end

    initial begin #20_000_000; $display("TB TIMEOUT"); $finish; end
endmodule
