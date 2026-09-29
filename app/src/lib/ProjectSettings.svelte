<script lang="ts">
  import { fromShown, lengthUnit, toShown } from './format';
  import Section from './Section.svelte';
  import { app, edit } from './state.svelte';
  import type { Project } from './types/project';

  const p = $derived(app.project!);
  const outlets = $derived(p.system.elements.filter((e) => e.type === 'outlet'));
  const num = (e: Event) => Number((e.currentTarget as HTMLInputElement).value);
  /** Where the value comes from, when the project says (published, approximate, unknown…). */
  const basis = (path: string) => p.basis?.[path];
  const firingOrder = $derived(p.engine.geometry.firing_order.join('-'));

  function setFiringOrder(text: string) {
    const order = text.split(/[^0-9]+/).filter(Boolean).map(Number);
    if (order.length) edit((q) => (q.engine.geometry.firing_order = order));
  }
</script>

{#snippet field(label: string, value: number, set: (q: Project, v: number) => void, unit: string, path = '', step = 'any')}
  <label class="field">
    <span class="label">
      {#if path && basis(path)}<i class="dot {basis(path)}" title="This value is {basis(path)?.replace('_', ' ')}"></i>{/if}
      {label}
    </span>
    <input type="number" {step} {value} onchange={(e) => edit((q) => set(q, num(e)))} />
    <span class="unit">{unit}</span>
  </label>
{/snippet}

<div class="settings" data-testid="project-settings">
  <div class="legend">
    {#each [['published', 'published'], ['measured', 'measured'], ['owner_set', 'set by you'], ['approximate', 'approximate'], ['unknown', 'unknown']] as [b, name]}
      <span><i class="dot {b}"></i>{name}</span>
    {/each}
  </div>
  <Section title="Project">
    <label class="field wide">
      <span class="label">Name</span>
      <input value={p.name} onchange={(e) => edit((q) => (q.name = e.currentTarget.value))} />
    </label>
    {#if p.notes}<p class="notes muted">{p.notes}</p>{/if}
  </Section>

  <Section title="Operating sweep">
    {@render field('From', p.operating.sweep_rpm[0], (q, v) => (q.operating.sweep_rpm[0] = v), 'rpm', '', '50')}
    {@render field('To', p.operating.sweep_rpm[1], (q, v) => (q.operating.sweep_rpm[1] = v), 'rpm', '', '50')}
    {@render field('Step', p.operating.sweep_rpm[2], (q, v) => (q.operating.sweep_rpm[2] = v), 'rpm', '', '10')}
    {@render field('Cruise from', p.operating.cruise_band_rpm[0], (q, v) => (q.operating.cruise_band_rpm[0] = v), 'rpm', 'operating.cruise_band_rpm', '50')}
    {@render field('Cruise to', p.operating.cruise_band_rpm[1], (q, v) => (q.operating.cruise_band_rpm[1] = v), 'rpm', 'operating.cruise_band_rpm', '50')}
    {@render field('Intake air', p.operating.intake_temperature_k, (q, v) => (q.operating.intake_temperature_k = v), 'K', 'operating.intake_temperature_k')}
    {@render field('Volumetric eff.', p.operating.volumetric_efficiency, (q, v) => (q.operating.volumetric_efficiency = v), '', 'operating.volumetric_efficiency')}
    {@render field('Heat retained', p.operating.heat_retained, (q, v) => (q.operating.heat_retained = v), '', 'operating.heat_retained')}
    <div class="table muted">
      Manifold pressure: {p.operating.map_kpa.map(([r, k]) => `${r} rpm ${k} kPa`).join(' · ')}
      {#if basis('operating.map_kpa')}<span class="basis {basis('operating.map_kpa')}">{basis('operating.map_kpa')}</span>{/if}
    </div>
  </Section>

  <Section title="Solver">
    {@render field('Cell length Δx', p.solver.dx_mm, (q, v) => (q.solver.dx_mm = v), 'mm')}
    {@render field('Courant number', p.solver.cfl, (q, v) => (q.solver.cfl = v), '≤ 0.8')}
    {@render field('Cycles, at least', p.solver.min_cycles, (q, v) => (q.solver.min_cycles = v), '', '', '1')}
    {@render field('Cycles, at most', p.solver.max_cycles, (q, v) => (q.solver.max_cycles = v), '', '', '1')}
    {@render field('Periodic within', p.solver.periodicity_tolerance * 100, (q, v) => (q.solver.periodicity_tolerance = v / 100), '%')}
    {@render field('Highest frequency', p.solver.max_frequency_hz, (q, v) => (q.solver.max_frequency_hz = v), 'Hz', '', '100')}
    {@render field('Wall roughness', p.solver.wall_roughness_mm, (q, v) => (q.solver.wall_roughness_mm = v), 'mm')}
    <label class="field">
      <span class="label">Samples per cycle</span>
      <select value={String(p.solver.samples_per_cycle)} onchange={(e) => edit((q) => (q.solver.samples_per_cycle = Number(e.currentTarget.value)))}>
        {#each [512, 1024, 2048, 4096] as n}<option value={String(n)}>{n}</option>{/each}
      </select>
    </label>
    <label class="field">
      <span class="label">Limiter</span>
      <select value={p.solver.limiter} onchange={(e) => edit((q) => (q.solver.limiter = e.currentTarget.value as Project['solver']['limiter']))}>
        <option value="van_leer_tvb">van Leer, TVB</option>
        <option value="van_leer">van Leer</option>
        <option value="minmod">minmod</option>
      </select>
    </label>
    <label class="field">
      <span class="label">Wall friction</span>
      <input type="checkbox" checked={p.solver.friction} onchange={(e) => edit((q) => (q.solver.friction = e.currentTarget.checked))} />
    </label>
    <label class="field">
      <span class="label">Wall heat</span>
      <select
        value={p.solver.wall_thermal.model}
        onchange={(e) => {
          const model = e.currentTarget.value;
          edit((q) => (q.solver.wall_thermal = model === 'fixed' ? { model, temperature_k: 600 } : { model: model as 'computed' | 'adiabatic' }));
        }}
      >
        <option value="computed">thermal model</option>
        <option value="fixed">fixed wall temperature</option>
        <option value="adiabatic">none (adiabatic)</option>
      </select>
    </label>
    {#if p.solver.wall_thermal.model === 'fixed'}
      {@const fixed = p.solver.wall_thermal}
      {@render field('Wall temperature', fixed.temperature_k, (q, v) => (q.solver.wall_thermal = { model: 'fixed', temperature_k: v }), 'K')}
    {/if}
  </Section>

  <Section title="Listening point" open={false}>
    <label class="field">
      <span class="label">Outlet</span>
      <select value={p.receiver.outlet ?? ''} onchange={(e) => edit((q) => (q.receiver.outlet = e.currentTarget.value || null))}>
        <option value="">first outlet</option>
        {#each outlets as o (o.id)}<option value={o.id}>{o.id}</option>{/each}
      </select>
    </label>
    {@render field('Distance', p.receiver.distance_m, (q, v) => (q.receiver.distance_m = v), 'm')}
    {@render field('Angle to the axis', p.receiver.angle_deg, (q, v) => (q.receiver.angle_deg = v), '°')}
  </Section>

  <Section title="Ambient and vehicle" open={false}>
    {@render field('Pressure', p.ambient.pressure_pa / 1000, (q, v) => (q.ambient.pressure_pa = v * 1000), 'kPa')}
    {@render field('Temperature', p.ambient.temperature_k, (q, v) => (q.ambient.temperature_k = v), 'K')}
    {@render field('Vehicle speed', p.ambient.vehicle_speed_kmh, (q, v) => (q.ambient.vehicle_speed_kmh = v), 'km/h', 'ambient.vehicle_speed_kmh')}
    {@render field('Underbody air', p.ambient.underbody_air_factor ?? 0.5, (q, v) => (q.ambient.underbody_air_factor = v), '× speed', 'ambient.underbody_air_factor')}
    {@render field('Ground height', toShown(p.ambient.ground_z_mm), (q, v) => (q.ambient.ground_z_mm = fromShown(v)), lengthUnit(), 'ambient.ground_z_mm')}
  </Section>

  <Section title="Engine" open={false}>
    <label class="field wide">
      <span class="label">Engine</span>
      <input value={p.engine.label} onchange={(e) => edit((q) => (q.engine.label = e.currentTarget.value))} />
    </label>
    {@render field('Bore', p.engine.geometry.bore_mm, (q, v) => (q.engine.geometry.bore_mm = v), 'mm', 'engine.geometry.bore_mm')}
    {@render field('Stroke', p.engine.geometry.stroke_mm, (q, v) => (q.engine.geometry.stroke_mm = v), 'mm', 'engine.geometry.stroke_mm')}
    {@render field('Rod', p.engine.geometry.rod_mm, (q, v) => (q.engine.geometry.rod_mm = v), 'mm', 'engine.geometry.rod_mm')}
    {@render field('Compression', p.engine.geometry.compression_ratio, (q, v) => (q.engine.geometry.compression_ratio = v), ': 1', 'engine.geometry.compression_ratio')}
    <label class="field">
      <span class="label">Firing order</span>
      <input value={firingOrder} onchange={(e) => setFiringOrder(e.currentTarget.value)} />
      <span class="unit"></span>
    </label>
    {@render field('Valves / cylinder', p.engine.valves.valves_per_cylinder, (q, v) => (q.engine.valves.valves_per_cylinder = v), '', 'engine.valves.valves_per_cylinder', '1')}
    {@render field('Valve diameter', p.engine.valves.valve_diameter_mm, (q, v) => (q.engine.valves.valve_diameter_mm = v), 'mm', 'engine.valves.valve_diameter_mm')}
    {@render field('Throat diameter', p.engine.valves.throat_diameter_mm, (q, v) => (q.engine.valves.throat_diameter_mm = v), 'mm', 'engine.valves.throat_diameter_mm')}
    {@render field('Discharge coeff.', p.engine.valves.discharge_coefficient, (q, v) => (q.engine.valves.discharge_coefficient = v), '', 'engine.valves.discharge_coefficient')}
    {@render field('Exhaust opens', p.engine.valves.evo_deg, (q, v) => (q.engine.valves.evo_deg = v), '° ATDC', 'engine.valves.evo_deg')}
    {@render field('Exhaust closes', p.engine.valves.evc_deg, (q, v) => (q.engine.valves.evc_deg = v), '° ATDC', 'engine.valves.evc_deg')}
    {@render field('Maximum lift', p.engine.valves.max_lift_mm, (q, v) => (q.engine.valves.max_lift_mm = v), 'mm', 'engine.valves.max_lift_mm')}
    {@render field('Polytropic n', p.engine.polytropic_exponent, (q, v) => (q.engine.polytropic_exponent = v), '', 'engine.polytropic_exponent')}
    {@render field('Turbine extraction', p.turbine.extraction_factor, (q, v) => (q.turbine.extraction_factor = v), '', 'turbine.extraction_factor')}
    {@render field('Fuel H/C', p.gas.fuel_h_to_c, (q, v) => (q.gas.fuel_h_to_c = v), '')}
    {@render field('Lambda', p.gas.lambda, (q, v) => (q.gas.lambda = v), '')}
  </Section>
</div>

<style>
  .field {
    display: grid;
    grid-template-columns: 124px minmax(0, 1fr) 46px;
    align-items: center;
    gap: 6px;
    margin: 3px 0;
  }

  .field.wide {
    grid-template-columns: 124px minmax(0, 1fr);
  }

  .field input[type='checkbox'] {
    justify-self: start;
  }

  .label {
    color: var(--muted);
    display: flex;
    align-items: center;
    gap: 5px;
    min-width: 0;
  }

  .unit {
    color: var(--faint);
    font-size: 11px;
  }

  .basis {
    font-size: 9.5px;
    padding: 0 4px;
    border-radius: 6px;
    border: 1px solid var(--line-2);
    color: var(--faint);
    text-transform: lowercase;
  }

  .basis.approximate,
  .basis.unknown {
    border-color: #5a4a22;
    color: var(--warn);
  }

  .basis.unknown {
    border-color: #6a2e35;
    color: var(--bad);
  }

  .basis.published,
  .basis.measured {
    border-color: #245236;
    color: var(--good);
  }

  /* Where a value comes from: a dot before its label, named on hover. */
  .dot {
    flex: none;
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--faint);
  }

  .dot.approximate,
  .dot.owner_set {
    background: var(--warn);
  }

  .dot.owner_set {
    background: var(--accent);
  }

  .dot.unknown {
    background: var(--bad);
  }

  .dot.published,
  .dot.measured {
    background: var(--good);
  }

  .legend {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 12px;
    padding: 8px 10px;
    font-size: 11px;
    color: var(--muted);
  }

  .legend span {
    display: flex;
    align-items: center;
    gap: 5px;
  }

  .label {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .notes,
  .table {
    font-size: 11px;
    margin-top: 6px;
  }
</style>
