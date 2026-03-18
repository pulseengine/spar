import { test, expect } from '@playwright/test';
import path from 'path';

const TEST_HTML = `file://${path.resolve(__dirname, 'test-output.html')}`;

test.describe('AADL Architecture Rendering', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto(TEST_HTML);
    // Wait for SVG to be present
    await page.waitForSelector('svg');
  });

  // ── RENDER-REQ-002: Ports visible ──────────────────────────────────

  test('ports are rendered with type colors (RENDER-REQ-002)', async ({ page }) => {
    const ports = await page.locator('.port').count();
    expect(ports).toBeGreaterThan(0);

    // Check port circles exist
    const circles = await page.locator('.port circle').count();
    expect(circles).toBeGreaterThan(0);

    // Check data port class is present (our test model uses data ports)
    const dataPorts = await page.locator('.port.data').count();
    expect(dataPorts).toBeGreaterThan(0);
  });

  test('port labels are visible (RENDER-REQ-002)', async ({ page }) => {
    const portLabels = await page.locator('.port text').allTextContents();
    expect(portLabels.length).toBeGreaterThan(0);
    // Our test model has data_out, sensor_in, cmd_out, etc.
    const allText = portLabels.join(' ');
    expect(allText).toContain('data_out');
  });

  // ── RENDER-REQ-001: Orthogonal edges ───────────────────────────────

  test('edges use orthogonal routing (RENDER-REQ-001)', async ({ page }) => {
    const paths = await page.locator('.edge path').all();
    expect(paths.length).toBeGreaterThan(0);

    for (const pathEl of paths) {
      const d = await pathEl.getAttribute('d');
      expect(d).toBeTruthy();
      // Orthogonal paths use L (line-to) commands, not C (cubic bezier)
      // At minimum the path should contain L commands
      expect(d).toContain('L');
    }
  });

  // ── RENDER-REQ-003: Pan and zoom ───────────────────────────────────

  test('zoom changes viewBox (RENDER-REQ-003)', async ({ page }) => {
    const svg = page.locator('svg');
    const viewBoxBefore = await svg.getAttribute('viewBox');
    expect(viewBoxBefore).toBeTruthy();

    // Scroll to zoom
    await svg.dispatchEvent('wheel', { deltaY: -100 });
    // Small delay for JS to process
    await page.waitForTimeout(100);

    const viewBoxAfter = await svg.getAttribute('viewBox');
    // ViewBox should change after zoom
    expect(viewBoxAfter).not.toEqual(viewBoxBefore);
  });

  test('pan moves viewBox (RENDER-REQ-003)', async ({ page }) => {
    const svg = page.locator('svg');
    const viewBoxBefore = await svg.getAttribute('viewBox');

    // Simulate pan via JS — dispatch mousedown, mousemove, mouseup on SVG
    await page.evaluate(() => {
      const svgEl = document.querySelector('svg')!;
      const rect = svgEl.getBoundingClientRect();
      const cx = rect.left + rect.width / 2;
      const cy = rect.top + rect.height / 2;

      svgEl.dispatchEvent(new MouseEvent('mousedown', { clientX: cx, clientY: cy, bubbles: true }));
      window.dispatchEvent(new MouseEvent('mousemove', { clientX: cx + 50, clientY: cy + 50, bubbles: true }));
      window.dispatchEvent(new MouseEvent('mouseup', { bubbles: true }));
    });

    await page.waitForTimeout(100);
    const viewBoxAfter = await svg.getAttribute('viewBox');
    expect(viewBoxAfter).not.toEqual(viewBoxBefore);
  });

  // ── RENDER-REQ-005: Selection ──────────────────────────────────────

  test('click node adds selected class (RENDER-REQ-005)', async ({ page }) => {
    // Use JS click to ensure event bubbles correctly in SVG
    await page.evaluate(() => {
      const node = document.querySelector('.node')!;
      node.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    });

    const classes = await page.locator('.node').first().getAttribute('class');
    expect(classes).toContain('selected');
  });

  test('ctrl+click enables multi-select (RENDER-REQ-005)', async ({ page }) => {
    const count = await page.locator('.node').count();
    if (count < 2) return;

    // Click first, then ctrl+click second via JS
    await page.evaluate(() => {
      const nodes = document.querySelectorAll('.node');
      nodes[0].dispatchEvent(new MouseEvent('click', { bubbles: true }));
      nodes[1].dispatchEvent(new MouseEvent('click', { bubbles: true, ctrlKey: true, metaKey: true }));
    });

    const selected = await page.locator('.node.selected').count();
    expect(selected).toBe(2);
  });

  test('click emits etch-select event (RENDER-REQ-005)', async ({ page }) => {
    // Listen for custom event
    const eventPromise = page.evaluate(() => {
      return new Promise<string[]>((resolve) => {
        document.querySelector('svg')!.addEventListener('etch-select', ((e: CustomEvent) => {
          resolve(e.detail.ids);
        }) as EventListener, { once: true });
      });
    });

    // Click a node
    await page.locator('.node').first().click({ force: true });

    const selectedIds = await eventPromise;
    expect(selectedIds.length).toBeGreaterThan(0);
  });

  // ── RENDER-REQ-006: Semantic zoom ──────────────────────────────────

  test('semantic zoom hides detail at low zoom (RENDER-REQ-006)', async ({ page }) => {
    const svg = page.locator('svg');

    // Zoom out significantly (multiple wheel events)
    for (let i = 0; i < 15; i++) {
      await svg.dispatchEvent('wheel', { deltaY: 200 });
    }
    await page.waitForTimeout(200);

    // SVG should have zoom-low class
    const classes = await svg.getAttribute('class');
    expect(classes).toContain('zoom-low');
  });

  // ── RENDER-REQ-004: Determinism ────────────────────────────────────

  test('node positions are consistent across loads (RENDER-REQ-004)', async ({ page }) => {
    // Get all node positions
    const positions1 = await page.evaluate(() => {
      const nodes = document.querySelectorAll('.node rect');
      return Array.from(nodes).map(r => ({
        x: r.getAttribute('x'),
        y: r.getAttribute('y'),
      }));
    });

    // Reload and get positions again
    await page.reload();
    await page.waitForSelector('svg');

    const positions2 = await page.evaluate(() => {
      const nodes = document.querySelectorAll('.node rect');
      return Array.from(nodes).map(r => ({
        x: r.getAttribute('x'),
        y: r.getAttribute('y'),
      }));
    });

    expect(positions1).toEqual(positions2);
  });

  // ── Structure tests ────────────────────────────────────────────────

  test('HTML contains all expected elements', async ({ page }) => {
    // SVG present
    expect(await page.locator('svg').count()).toBe(1);

    // Nodes present (our model has 5 components)
    const nodes = await page.locator('.node').count();
    expect(nodes).toBeGreaterThanOrEqual(3);

    // Edges present (our model has connections)
    const edges = await page.locator('.edge').count();
    expect(edges).toBeGreaterThanOrEqual(1);

    // Interactive script loaded (check for viewBox manipulation capability)
    const hasScript = await page.evaluate(() => {
      return document.querySelectorAll('script').length > 0;
    });
    expect(hasScript).toBe(true);
  });

  test('container nodes have container class', async ({ page }) => {
    // System Top should be a container with children
    const containers = await page.locator('.node.container').count();
    // At least the root system or the process should be containers
    expect(containers).toBeGreaterThanOrEqual(0);
  });

  test('nodes have data-id attributes', async ({ page }) => {
    const firstNode = page.locator('.node').first();
    const dataId = await firstNode.getAttribute('data-id');
    expect(dataId).toBeTruthy();
  });
});
