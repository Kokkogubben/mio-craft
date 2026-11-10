package dev.claudelauncher.live;

import java.io.Reader;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

import com.google.gson.Gson;

/**
 * Definitions written by the launcher to {@code <live dir>/content.json}.
 * Re-read whenever the file changes; readers always see a complete snapshot.
 */
public final class LiveContent {
    public static final class Drop {
        public String item;
        public int min = 1;
        public int max = 1;
    }

    public static final class Behaviour {
        /** Folder of compiled classes (a mod project's build/classes/java/main). */
        public String classes_dir;
        /** Fully qualified class implementing the matching behaviour interface. */
        public String class_name;
    }

    public static final class BlockDef {
        public int slot = -1;
        /** The content's own id, e.g. "ruby_ore" (for promotion to a real registration later). */
        public String id;
        public String name;
        /** PNG path relative to the live dir, 16×16. */
        public String texture;
        public float hardness = 1.5f;
        public float resistance = 6.0f;
        public int light = 0;
        public boolean requires_tool = false;
        public List<Drop> drops = new ArrayList<>();
        public Behaviour behaviour;
        /** Route 4: register for real under this id (namespace:path) instead of using a slot. */
        public String register_as;
    }

    public static final class ItemDef {
        public int slot = -1;
        public String id;
        public String name;
        public String texture;
        public int max_stack = 64;
        public Behaviour behaviour;
        public String register_as;
    }

    public int version;
    public List<BlockDef> blocks = new ArrayList<>();
    public List<ItemDef> items = new ArrayList<>();

    private static volatile LiveContent current = new LiveContent();
    private static long lastModified = -1;
    private static final Gson GSON = new Gson();

    public static LiveContent current() {
        return current;
    }

    public static Path file() {
        return ClaudeLive.DIR.resolve("content.json");
    }

    /** Re-read content.json if it changed. Returns true when the content changed. */
    public static synchronized boolean reloadIfChanged() {
        Path f = file();
        try {
            if (!Files.exists(f)) return false;
            long m = Files.getLastModifiedTime(f).toMillis();
            if (m == lastModified) return false;
            try (Reader r = Files.newBufferedReader(f, StandardCharsets.UTF_8)) {
                LiveContent c = GSON.fromJson(r, LiveContent.class);
                if (c == null) return false;
                if (c.blocks == null) c.blocks = new ArrayList<>();
                if (c.items == null) c.items = new ArrayList<>();
                current = c;
            }
            lastModified = m;
            return true;
        } catch (Exception e) {
            // The launcher writes atomically, so this is a real problem with the file.
            ClaudeLive.LOG.warn("Couldn't read {}: {}", f, e.toString());
            return false;
        }
    }

    public BlockDef block(int slot) {
        for (BlockDef d : blocks) if (d.slot == slot && d.register_as == null) return d;
        return null;
    }

    public ItemDef item(int slot) {
        for (ItemDef d : items) if (d.slot == slot && d.register_as == null) return d;
        return null;
    }

    public List<BlockDef> registeredBlocks() {
        List<BlockDef> out = new ArrayList<>();
        for (BlockDef d : blocks) if (d.register_as != null) out.add(d);
        return Collections.unmodifiableList(out);
    }

    public List<ItemDef> registeredItems() {
        List<ItemDef> out = new ArrayList<>();
        for (ItemDef d : items) if (d.register_as != null) out.add(d);
        return Collections.unmodifiableList(out);
    }
}
