package dev.miocraft.live;

import dev.miocraft.live.api.LiveItemBehaviour;
import net.minecraft.network.chat.Component;
import net.minecraft.world.InteractionHand;
import net.minecraft.world.InteractionResult;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.item.Item;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.level.Level;

/** A pre-registered item slot with a live name and behaviour. */
public class LiveItem extends Item {
    /** Slot number, or -1 for content registered at runtime (route 4). */
    public final int slot;
    public final String registeredAs;

    public LiveItem(int slot, Properties props) {
        this(slot, null, props);
    }

    public LiveItem(int slot, String registeredAs, Properties props) {
        super(props);
        this.slot = slot;
        this.registeredAs = registeredAs;
    }

    public LiveContent.ItemDef def() {
        LiveContent c = LiveContent.current();
        if (registeredAs == null) return c.item(slot);
        for (LiveContent.ItemDef d : c.registeredItems()) if (registeredAs.equals(d.register_as)) return d;
        return null;
    }

    @Override
    public Component getName(ItemStack stack) { // VERIFY(26.x)
        LiveContent.ItemDef d = def();
        return Component.literal(d != null && d.name != null ? d.name
                : registeredAs != null ? registeredAs : "Empty live item " + slot);
    }

    // VERIFY(26.x): Item#use returns InteractionResult since 1.21.2
    @Override
    public InteractionResult use(Level level, Player player, InteractionHand hand) {
        LiveContent.ItemDef d = def();
        LiveItemBehaviour b = d == null ? null : Behaviours.get(d.behaviour, LiveItemBehaviour.class);
        if (b == null) return super.use(level, player, hand);
        try {
            return b.use(level, player, hand);
        } catch (Throwable t) {
            Behaviours.report(t);
            return InteractionResult.PASS;
        }
    }
}
