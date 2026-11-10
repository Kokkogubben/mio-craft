package dev.claudelauncher.live;

import java.io.InputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

import com.mojang.blaze3d.platform.NativeImage;
import com.mojang.blaze3d.systems.RenderSystem;
import com.mojang.blaze3d.textures.GpuTexture;
import net.fabricmc.loader.api.FabricLoader;
import net.minecraft.client.Minecraft;
import net.minecraft.client.renderer.texture.TextureAtlas;
import net.minecraft.client.renderer.texture.TextureAtlasSprite;
import net.minecraft.data.AtlasIds;
import net.minecraft.resources.Identifier;
import net.minecraft.server.packs.repository.PackRepository;

/**
 * Applies live textures.
 *
 * <p><b>In-memory mode</b> paints each texture straight into the block atlas at
 * the spot reserved for its slot at startup (every slot ships a placeholder
 * texture, so the atlas already has room). No reload; normal lighting and
 * performance. Repainted automatically after any resource reload.
 *
 * <p><b>Resource pack mode</b> copies textures into the {@code claude-live}
 * resource pack and triggers one automatic resource reload (like F3+T).
 * Runtime-registered content (route 4) always uses this, because its models
 * are new.
 */
public final class Textures {
    private Textures() {}

    /** sprite id → (texture file mtime, sprite object it was painted into). */
    private static final Map<String, Object[]> PAINTED = new HashMap<>();
    private static final Map<String, Long> COPIED = new HashMap<>();
    private static long lastPackVersion = -1;

    public static Path packDir() {
        return FabricLoader.getInstance().getGameDir().resolve("resourcepacks").resolve("claude-live");
    }

    public static void tick(Minecraft mc) {
        LiveContent c = LiveContent.current();
        if (ClaudeLive.IN_MEMORY_TEXTURES) {
            for (LiveContent.BlockDef d : c.blocks) {
                if (d.register_as == null && d.texture != null) paint(mc, ClaudeLive.id("block/slot_block_" + d.slot), d.texture);
            }
            for (LiveContent.ItemDef d : c.items) {
                if (d.register_as == null && d.texture != null) paint(mc, ClaudeLive.id("item/slot_item_" + d.slot), d.texture);
            }
        } else {
            boolean changed = false;
            for (LiveContent.BlockDef d : c.blocks) {
                if (d.register_as == null && d.texture != null) changed |= copy(d.texture, "assets/claude_live/textures/block/slot_block_" + d.slot + ".png");
            }
            for (LiveContent.ItemDef d : c.items) {
                if (d.register_as == null && d.texture != null) changed |= copy(d.texture, "assets/claude_live/textures/item/slot_item_" + d.slot + ".png");
            }
            if (changed) reloadPack(mc);
        }
        // Route 4 content: the launcher writes its assets straight into the pack and bumps pack.version.
        long v = packVersion();
        if (v != lastPackVersion) {
            boolean first = lastPackVersion == -1;
            lastPackVersion = v;
            if (!first && v > 0) reloadPack(mc);
        }
    }

    private static long packVersion() {
        try {
            Path f = packDir().resolve("pack.version");
            return Files.exists(f) ? Long.parseLong(Files.readString(f).trim()) : 0;
        } catch (Exception e) {
            return 0;
        }
    }

    private static void paint(Minecraft mc, Identifier spriteId, String texture) {
        Path file = ClaudeLive.DIR.resolve(texture);
        try {
            if (!Files.exists(file)) return;
            long mtime = Files.getLastModifiedTime(file).toMillis();
            // VERIFY(26.x): AtlasManager / AtlasIds replaced ModelManager#getAtlas in 1.21.9
            TextureAtlas atlas = mc.getAtlasManager().getAtlasOrThrow(AtlasIds.BLOCKS);
            TextureAtlasSprite sprite = atlas.getSprite(spriteId);
            Object[] prev = PAINTED.get(spriteId.toString());
            // Skip when unchanged and the atlas wasn't rebuilt (same sprite object).
            if (prev != null && (long) prev[0] == mtime && prev[1] == sprite) return;

            int w = sprite.contents().width(), h = sprite.contents().height(); // VERIFY(26.x)
            NativeImage src;
            try (InputStream in = Files.newInputStream(file)) {
                src = NativeImage.read(in);
            }
            NativeImage img = fit(src, w, h);
            GpuTexture gpu = atlas.getTexture(); // VERIFY(26.x): AbstractTexture#getTexture
            int levels = gpu.getMipLevels(); // VERIFY(26.x)
            NativeImage level = img;
            for (int m = 0; m < levels; m++) {
                int lw = Math.max(1, w >> m), lh = Math.max(1, h >> m);
                if (m > 0) level = downscale(level, lw, lh);
                // VERIFY(26.x): CommandEncoder#writeToTexture parameters (depth/layer argument added in 1.21.6)
                RenderSystem.getDevice().createCommandEncoder().writeToTexture(
                        gpu, level, m, 0, sprite.getX() >> m, sprite.getY() >> m, lw, lh, 0, 0);
            }
            PAINTED.put(spriteId.toString(), new Object[] {mtime, sprite});
        } catch (Throwable t) {
            ClaudeLive.LOG.warn("Couldn't paint live texture {} into the atlas: {}", spriteId, t.toString());
            PAINTED.put(spriteId.toString(), new Object[] {Long.MIN_VALUE, null}); // don't retry every tick
        }
    }

    /** Nearest-neighbour resize (pixel art stays crisp). */
    private static NativeImage fit(NativeImage src, int w, int h) {
        if (src.getWidth() == w && src.getHeight() == h) return src;
        NativeImage out = new NativeImage(w, h, true);
        for (int y = 0; y < h; y++) {
            for (int x = 0; x < w; x++) {
                out.setPixel(x, y, src.getPixel(x * src.getWidth() / w, y * src.getHeight() / h)); // VERIFY(26.x): getPixel/setPixel (ARGB)
            }
        }
        return out;
    }

    /** 2×2 box filter for mipmap levels. */
    private static NativeImage downscale(NativeImage src, int w, int h) {
        NativeImage out = new NativeImage(w, h, true);
        for (int y = 0; y < h; y++) {
            for (int x = 0; x < w; x++) {
                int[] c = new int[4];
                int n = 0;
                for (int dy = 0; dy < 2; dy++) {
                    for (int dx = 0; dx < 2; dx++) {
                        int sx = Math.min(src.getWidth() - 1, x * 2 + dx), sy = Math.min(src.getHeight() - 1, y * 2 + dy);
                        int p = src.getPixel(sx, sy);
                        c[0] += (p >>> 24) & 0xff;
                        c[1] += (p >>> 16) & 0xff;
                        c[2] += (p >>> 8) & 0xff;
                        c[3] += p & 0xff;
                        n++;
                    }
                }
                out.setPixel(x, y, ((c[0] / n) << 24) | ((c[1] / n) << 16) | ((c[2] / n) << 8) | (c[3] / n));
            }
        }
        return out;
    }

    private static boolean copy(String texture, String packPath) {
        Path file = ClaudeLive.DIR.resolve(texture);
        try {
            if (!Files.exists(file)) return false;
            long mtime = Files.getLastModifiedTime(file).toMillis();
            Long prev = COPIED.get(packPath);
            if (prev != null && prev == mtime) return false;
            Path dst = packDir().resolve(packPath);
            Files.createDirectories(dst.getParent());
            Files.copy(file, dst, StandardCopyOption.REPLACE_EXISTING);
            COPIED.put(packPath, mtime);
            return true;
        } catch (Exception e) {
            ClaudeLive.LOG.warn("Couldn't copy live texture {}: {}", file, e.toString());
            return false;
        }
    }

    private static long lastReload;

    /** Make sure the claude-live pack exists and is enabled, then reload resources (debounced). */
    public static void reloadPack(Minecraft mc) {
        long now = System.currentTimeMillis();
        if (now - lastReload < 2000) return;
        lastReload = now;
        try {
            Path dir = packDir();
            Files.createDirectories(dir);
            Path meta = dir.resolve("pack.mcmeta");
            if (!Files.exists(meta)) {
                // VERIFY(26.x): pack.mcmeta uses min_format/max_format since 1.21.9
                Files.writeString(meta, "{\"pack\":{\"description\":\"Claude Live textures\",\"pack_format\":64,\"min_format\":1,\"max_format\":10000}}", StandardCharsets.UTF_8);
            }
            PackRepository repo = mc.getResourcePackRepository();
            repo.reload();
            String id = "file/claude-live";
            if (!repo.getSelectedIds().contains(id)) {
                List<String> ids = new ArrayList<>(repo.getSelectedIds());
                ids.add(id);
                repo.setSelected(ids); // VERIFY(26.x)
                mc.options.updateResourcePacks(repo); // VERIFY(26.x): saves options and reloads
                return;
            }
            mc.reloadResourcePacks();
        } catch (Throwable t) {
            ClaudeLive.LOG.warn("Couldn't reload the Claude Live resource pack: {}", t.toString());
        }
    }
}
