// Self-checking testbench written for the ZynqSim universality corpus (not upstream code).
// uart_tx drives uart_rx through one wire; every byte sent must come out of the receiver in
// order, with no framing or overrun error.
`timescale 1ns / 1ps

module corpus_tb;
    reg clk = 0;
    reg rst = 1;
    always #5 clk = ~clk;

    reg  [7:0] tx_data = 8'h00;
    reg        tx_valid = 1'b0;
    wire       tx_ready;
    wire       line;
    wire       tx_busy;
    wire [7:0] rx_data;
    wire       rx_valid;
    wire       rx_busy, rx_overrun, rx_frame;
    localparam [15:0] PRESCALE = 16'd2;   // 16 clocks per bit

    uart_tx #(.DATA_WIDTH(8)) u_tx (
        .clk(clk), .rst(rst),
        .s_axis_tdata(tx_data), .s_axis_tvalid(tx_valid), .s_axis_tready(tx_ready),
        .txd(line), .busy(tx_busy), .prescale(PRESCALE));

    uart_rx #(.DATA_WIDTH(8)) u_rx (
        .clk(clk), .rst(rst),
        .m_axis_tdata(rx_data), .m_axis_tvalid(rx_valid), .m_axis_tready(1'b1),
        .rxd(line), .busy(rx_busy), .overrun_error(rx_overrun), .frame_error(rx_frame),
        .prescale(PRESCALE));

    localparam N = 8;
    reg [7:0] expected [0:N-1];
    integer sent = 0;
    integer got = 0;
    integer errors = 0;

    initial begin
        expected[0] = 8'h55; expected[1] = 8'hA3; expected[2] = 8'h00; expected[3] = 8'hFF;
        expected[4] = 8'h01; expected[5] = 8'h80; expected[6] = 8'h7E; expected[7] = 8'hC9;
    end

    always @(posedge clk) begin
        if (rx_valid) begin
            if (got < N && rx_data !== expected[got]) begin
                $display("byte %0d: got %02x expected %02x", got, rx_data, expected[got]);
                errors = errors + 1;
            end
            got = got + 1;
        end
        if (rx_overrun || rx_frame) begin
            $display("receiver error: overrun=%b frame=%b", rx_overrun, rx_frame);
            errors = errors + 1;
        end
    end

    initial begin
        repeat (4) @(posedge clk);
        rst <= 1'b0;
        repeat (4) @(posedge clk);
        while (sent < N) begin
            tx_data  <= expected[sent];
            tx_valid <= 1'b1;
            @(posedge clk);
            while (!(tx_valid && tx_ready)) @(posedge clk);
            tx_valid <= 1'b0;
            sent = sent + 1;
            @(posedge clk);
        end
        // 8 bytes at 10 bit times of 16 clocks each are long done after this.
        repeat (400) @(posedge clk);
        if (got !== N) begin
            $display("received %0d bytes, expected %0d", got, N);
            errors = errors + 1;
        end
        if (errors == 0) $display("CORPUS_PASS forencich-uart");
        else $display("CORPUS_FAIL forencich-uart (%0d errors)", errors);
        $finish;
    end

    // A stuck design must not hang the run.
    initial begin
        #200000;
        $display("CORPUS_FAIL forencich-uart (timeout)");
        $finish;
    end
endmodule
