package dev.miocraft.live;

import java.io.IOException;
import java.net.URL;
import java.net.URLClassLoader;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.HashMap;
import java.util.Map;
import java.util.stream.Stream;

/**
 * Loads behaviour classes from a mod project's compiled classes folder, in a
 * fresh class loader each time the folder changes. That allows completely new
 * classes, not just edited method bodies, without restarting.
 *
 * <p>The loader's parent is this mod's class loader, so behaviour code sees
 * Minecraft and the behaviour interfaces. Classes from the same folder that are
 * also inside a mod jar are loaded child-first, so the newest version wins.
 */
public final class Behaviours {
    private Behaviours() {}

    private static final class Entry {
        final URLClassLoader loader;
        final long stamp;
        final Map<String, Object> instances = new HashMap<>();

        Entry(URLClassLoader loader, long stamp) {
            this.loader = loader;
            this.stamp = stamp;
        }
    }

    private static final Map<String, Entry> LOADERS = new HashMap<>();
    private static String lastError;

    public static synchronized <T> T get(LiveContent.Behaviour b, Class<T> type) {
        if (b == null || b.classes_dir == null || b.class_name == null) return null;
        try {
            Entry e = LOADERS.get(b.classes_dir);
            if (e == null) {
                e = open(b.classes_dir);
                LOADERS.put(b.classes_dir, e);
            }
            Object o = e.instances.get(b.class_name);
            if (o == null) {
                Class<?> c = Class.forName(b.class_name, true, e.loader);
                o = c.getDeclaredConstructor().newInstance();
                e.instances.put(b.class_name, o);
            }
            return type.isInstance(o) ? type.cast(o) : null;
        } catch (Throwable t) {
            report(t);
            return null;
        }
    }

    /** Drop every loader so the next use loads the latest classes. */
    public static synchronized void invalidate() {
        for (Entry e : LOADERS.values()) {
            try {
                e.loader.close();
            } catch (IOException ignored) {
            }
        }
        LOADERS.clear();
    }

    /** Called periodically: reopen loaders whose class folders changed. */
    public static synchronized void reloadIfClassesChanged() {
        boolean stale = false;
        for (Map.Entry<String, Entry> e : LOADERS.entrySet()) {
            if (stamp(e.getKey()) != e.getValue().stamp) {
                stale = true;
                break;
            }
        }
        if (stale) {
            invalidate();
            ClaudeLive.LOG.info("Reloaded live behaviour classes");
        }
    }

    private static Entry open(String dir) throws Exception {
        Path p = Paths.get(dir);
        URL[] urls = {p.toUri().toURL()};
        ClassLoader parent = Behaviours.class.getClassLoader();
        URLClassLoader loader = new URLClassLoader("claude-live-behaviours", urls, parent) {
            @Override
            protected Class<?> loadClass(String name, boolean resolve) throws ClassNotFoundException {
                // Child-first for the project's own classes, parent-first for everything else.
                synchronized (getClassLoadingLock(name)) {
                    Class<?> c = findLoadedClass(name);
                    if (c == null && findResource(name.replace('.', '/') + ".class") != null) {
                        c = findClass(name);
                    }
                    if (c == null) return super.loadClass(name, resolve);
                    if (resolve) resolveClass(c);
                    return c;
                }
            }
        };
        return new Entry(loader, stamp(dir));
    }

    /** Newest modification time of any .class file under dir. */
    private static long stamp(String dir) {
        try (Stream<Path> s = Files.walk(Paths.get(dir))) {
            return s.filter(f -> f.toString().endsWith(".class"))
                    .mapToLong(f -> {
                        try {
                            return Files.getLastModifiedTime(f).toMillis();
                        } catch (IOException e) {
                            return 0L;
                        }
                    })
                    .max()
                    .orElse(0L);
        } catch (IOException e) {
            return -1;
        }
    }

    /** Log a behaviour error once (so a broken behaviour doesn't flood the log every tick). */
    public static void report(Throwable t) {
        String msg = t.toString();
        if (!msg.equals(lastError)) {
            lastError = msg;
            ClaudeLive.LOG.error("Live behaviour failed", t);
        }
    }
}
