-- Written for the ZynqSim universality corpus (not third-party code).
entity top is end entity;
architecture sim of top is
begin
  process begin
    wait for 10 ns
    wait;
  end process;
end architecture;
