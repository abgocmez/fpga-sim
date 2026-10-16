-- Written for the ZynqSim universality corpus (not third-party code).
library ieee;
use ieee.std_logic_1164.all;
entity top is end entity;
architecture sim of top is
  signal a, b, y : std_logic := '0';
begin
  process (all) begin
    y <= a and b;
  end process;
  process begin
    wait for 10 ns;
    report "CORPUS_PASS vhdl_2008_construct";
    wait;
  end process;
end architecture;
