<script lang="ts">
  import { onMount } from 'svelte';
  import * as THREE from 'three';
  import { OrbitControls } from 'three/examples/jsm/controls/OrbitControls.js';
  import { TransformControls } from 'three/examples/jsm/controls/TransformControls.js';
  import { Line2 } from 'three/examples/jsm/lines/Line2.js';
  import { LineGeometry } from 'three/examples/jsm/lines/LineGeometry.js';
  import { LineMaterial } from 'three/examples/jsm/lines/LineMaterial.js';
  import { app, edit, type Selection } from './state.svelte';
  import type { Layout, PieceLayout } from './types/api';

  type Vec3 = [number, number, number];
  type Pick = Selection;

  // Colours by pipe material family; the selected route in the accent colour.
  const MATERIAL_COLOUR: Record<string, number> = { '409': 0x8d949e, '304': 0xb9c2cc, '321': 0xb9c2cc, 'ti-gr5': 0x9d95c9 };
  const ACCENT = 0x6aa9ff;

  let host: HTMLDivElement;
  let renderer: THREE.WebGLRenderer;
  let scene: THREE.Scene;
  let camera: THREE.PerspectiveCamera;
  let orbit: OrbitControls;
  let gizmo: TransformControls;
  let content = new THREE.Group();
  let handles = new Map<string, THREE.Mesh>();
  let lineMaterials: LineMaterial[] = [];
  let framed = false;

  const v3 = (p: readonly number[]) => new THREE.Vector3(p[0], p[1], p[2]);
  const key = (route: string, index: number) => `${route}#${index}`;

  /** Mesh of a coaxial frustum from `a` (diameter `da`) to `b` (diameter `db`). */
  function frustum(a: THREE.Vector3, b: THREE.Vector3, da: number, db: number, material: THREE.Material) {
    const axis = b.clone().sub(a);
    const mesh = new THREE.Mesh(new THREE.CylinderGeometry(db / 2, da / 2, axis.length(), 32, 1), material);
    mesh.quaternion.setFromUnitVectors(new THREE.Vector3(0, 1, 0), axis.clone().normalize());
    mesh.position.copy(a.clone().add(b).multiplyScalar(0.5));
    return mesh;
  }

  /** Points along a centreline piece (arcs every ~5°). */
  function samples(piece: PieceLayout): THREE.Vector3[] {
    if (piece.kind === 'straight') return [v3(piece.from), v3(piece.to)];
    const c = v3(piece.centre);
    const u = v3(piece.from).sub(c).normalize();
    const w = v3(piece.to).sub(c);
    const v = w.clone().sub(u.clone().multiplyScalar(u.dot(w))).normalize();
    const theta = (piece.angle_deg * Math.PI) / 180;
    const n = Math.max(2, Math.ceil(piece.angle_deg / 5));
    return Array.from({ length: n + 1 }, (_, i) => {
      const t = (theta * i) / n;
      return c.clone().add(u.clone().multiplyScalar(piece.radius_mm * Math.cos(t))).add(v.clone().multiplyScalar(piece.radius_mm * Math.sin(t)));
    });
  }

  function clear() {
    gizmo?.detach();
    content.traverse((o) => {
      if (o instanceof THREE.Mesh || o instanceof Line2) {
        o.geometry.dispose();
        (Array.isArray(o.material) ? o.material : [o.material]).forEach((m) => m.dispose());
      }
    });
    scene.remove(content);
    content = new THREE.Group();
    handles = new Map();
    lineMaterials = [];
  }

  function build(layout: Layout, selection: Selection | null) {
    clear();
    const selectedRoute = selection?.kind === 'route' ? selection.id : selection?.kind === 'vertex' ? selection.route : null;
    for (const r of layout.routes) {
      const colour = r.id === selectedRoute ? ACCENT : (MATERIAL_COLOUR[r.material] ?? 0x8d949e);
      const material = new THREE.MeshStandardMaterial({ color: colour, metalness: 0.55, roughness: 0.45 });
      const pick: Pick = { kind: 'route', id: r.id };
      const centreline: THREE.Vector3[] = [];
      for (const piece of r.pieces) {
        const pts = samples(piece);
        centreline.push(...(centreline.length ? pts.slice(1) : pts));
        const mesh =
          piece.kind === 'straight'
            ? frustum(pts[0], pts[1], r.od_mm, r.od_mm, material)
            : new THREE.Mesh(new THREE.TubeGeometry(new THREE.CatmullRomCurve3(pts), pts.length * 2, r.od_mm / 2, 24, false), material);
        mesh.userData.pick = pick;
        content.add(mesh);
      }
      // The flow solver's x-axis, drawn over the pipe.
      const geometry = new LineGeometry();
      geometry.setPositions(centreline.flatMap((p) => [p.x, p.y, p.z]));
      const lm = new LineMaterial({ color: 0xffd479, linewidth: 1.5, depthTest: false, transparent: true, opacity: 0.8 });
      lineMaterials.push(lm);
      const line = new Line2(geometry, lm);
      line.renderOrder = 2;
      content.add(line);
      r.points.slice(1, -1).forEach((p, i) => {
        const chosen = selection?.kind === 'vertex' && selection.route === r.id && selection.index === i;
        const handle = new THREE.Mesh(
          new THREE.SphereGeometry(Math.max(0.45 * r.od_mm, 10), 20, 14),
          new THREE.MeshStandardMaterial({ color: chosen ? 0xffffff : 0xf5a623, emissive: chosen ? 0x553300 : 0x000000 }),
        );
        handle.position.copy(v3(p));
        handle.userData.pick = { kind: 'vertex', route: r.id, index: i } satisfies Pick;
        handle.renderOrder = 3;
        handles.set(key(r.id, i), handle);
        content.add(handle);
      });
    }
    for (const e of layout.elements) {
      const chosen = selection?.kind === 'element' && selection.id === e.id;
      const material = new THREE.MeshStandardMaterial({
        color: chosen ? ACCENT : 0x4f6b8a,
        metalness: 0.3,
        roughness: 0.6,
        transparent: true,
        opacity: 0.6,
      });
      const pick: Pick = { kind: 'element', id: e.id };
      for (const f of e.body) {
        const mesh = frustum(v3(f.from), v3(f.to), f.d_from_mm, f.d_to_mm, material);
        mesh.userData.pick = pick;
        content.add(mesh);
      }
      for (const p of e.ports) {
        const port = new THREE.Mesh(new THREE.SphereGeometry(7, 12, 8), new THREE.MeshBasicMaterial({ color: 0x5fd08a }));
        port.position.copy(v3(p.position));
        port.userData.pick = pick;
        content.add(port);
      }
    }
    scene.add(content);
    if (selection?.kind === 'vertex') {
      const handle = handles.get(key(selection.route, selection.index));
      if (handle) gizmo.attach(handle);
    }
    resize();
    if (!framed) frame();
  }

  /** Fits the whole system in view, seen from the car's right side, slightly from above. */
  function frame() {
    const box = new THREE.Box3().setFromObject(content);
    if (box.isEmpty()) return;
    const sphere = box.getBoundingSphere(new THREE.Sphere());
    const fov = (camera.fov * Math.PI) / 180;
    const narrowest = Math.min(fov, 2 * Math.atan(Math.tan(fov / 2) * camera.aspect));
    const distance = (1.05 * sphere.radius) / Math.sin(narrowest / 2);
    orbit.target.copy(sphere.center);
    camera.position.copy(sphere.center).addScaledVector(new THREE.Vector3(0.25, -0.85, 0.45).normalize(), distance);
    camera.near = distance / 200;
    camera.far = distance * 50;
    camera.updateProjectionMatrix();
    orbit.update();
    framed = true;
  }

  function resize() {
    if (!host || !renderer) return;
    const { clientWidth: w, clientHeight: h } = host;
    renderer.setSize(w, h, false);
    camera.aspect = w / Math.max(h, 1);
    camera.updateProjectionMatrix();
    lineMaterials.forEach((m) => m.resolution.set(w, h));
  }

  /** A via point dragged to a new place becomes an edit of its route. */
  function commitDrag() {
    const object = gizmo.object;
    const pick = object?.userData.pick as Pick | undefined;
    if (!object || pick?.kind !== 'vertex') return;
    const p = object.position;
    const at: Vec3 = [Math.round(p.x), Math.round(p.y), Math.round(p.z)];
    edit((project) => {
      const r = project.system.routes.find((x) => x.id === pick.route);
      if (r?.via_mm) r.via_mm[pick.index] = at;
    });
  }

  onMount(() => {
    renderer = new THREE.WebGLRenderer({ antialias: true });
    renderer.setPixelRatio(window.devicePixelRatio);
    host.appendChild(renderer.domElement);
    scene = new THREE.Scene();
    scene.background = new THREE.Color(0x11151d);
    camera = new THREE.PerspectiveCamera(35, 1, 5, 200000);
    camera.up.set(0, 0, 1);
    camera.position.set(2500, 3000, 1800);
    orbit = new OrbitControls(camera, renderer.domElement);
    orbit.enableDamping = true;
    scene.add(new THREE.HemisphereLight(0xdde6ff, 0x1a1f28, 1.4));
    const sun = new THREE.DirectionalLight(0xffffff, 1.8);
    sun.position.set(1500, 2000, 4000);
    scene.add(sun);
    const axes = new THREE.AxesHelper(250);
    scene.add(axes);
    gizmo = new TransformControls(camera, renderer.domElement);
    gizmo.setSize(0.8);
    gizmo.addEventListener('dragging-changed', (e) => {
      orbit.enabled = !e.value;
      if (!e.value) commitDrag();
    });
    scene.add(gizmo.getHelper());

    let down: { x: number; y: number } | null = null;
    const raycaster = new THREE.Raycaster();
    renderer.domElement.addEventListener('pointerdown', (e) => (down = { x: e.clientX, y: e.clientY }));
    renderer.domElement.addEventListener('pointerup', (e) => {
      if (!down || gizmo.dragging || Math.hypot(e.clientX - down.x, e.clientY - down.y) > 4) return;
      const rect = renderer.domElement.getBoundingClientRect();
      const ndc = new THREE.Vector2(((e.clientX - rect.left) / rect.width) * 2 - 1, -((e.clientY - rect.top) / rect.height) * 2 + 1);
      raycaster.setFromCamera(ndc, camera);
      const hit = raycaster.intersectObjects(content.children, false).find((h) => h.object.userData.pick);
      app.selection = (hit?.object.userData.pick as Pick | undefined) ?? null;
    });

    const observer = new ResizeObserver(resize);
    observer.observe(host);
    renderer.setAnimationLoop(() => {
      orbit.update();
      renderer.render(scene, camera);
    });
    return () => {
      observer.disconnect();
      renderer.setAnimationLoop(null);
      clear();
      gizmo.dispose();
      orbit.dispose();
      renderer.dispose();
    };
  });

  $effect(() => {
    const layout = app.layout;
    const selection = app.selection;
    if (renderer && layout) build(layout, selection);
  });

  // A new project is framed afresh.
  $effect(() => {
    void app.path;
    void app.project?.name;
    framed = false;
  });

  $effect(() => {
    const ground = app.project?.ambient.ground_z_mm;
    if (!scene || ground === undefined) return;
    const grid = new THREE.GridHelper(8000, 80, 0x2f3949, 0x222a36);
    grid.rotation.x = Math.PI / 2;
    grid.position.z = ground;
    scene.add(grid);
    return () => {
      scene.remove(grid);
      grid.geometry.dispose();
      (grid.material as THREE.Material).dispose();
    };
  });
</script>

<div class="viewport" bind:this={host} data-testid="viewport">
  <div class="legend muted">
    X forward · Y left · Z up · mm · origin at the downpipe flange<br />
    Click to select · drag a via point's arrows to move it
  </div>
</div>

<style>
  .viewport {
    position: absolute;
    inset: 0;
  }

  .viewport :global(canvas) {
    display: block;
    width: 100%;
    height: 100%;
  }

  .legend {
    position: absolute;
    left: 10px;
    top: 8px;
    font-size: 11px;
    pointer-events: none;
  }
</style>
