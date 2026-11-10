package dev.miocraft.live;

import net.minecraft.network.chat.Component;
import net.minecraft.world.item.BlockItem;
import net.minecraft.world.item.ItemStack;

/** The item form of a {@link LiveBlock}; its name follows the block's live name. */
public class LiveBlockItem extends BlockItem {
    public LiveBlockItem(LiveBlock block, Properties props) {
        super(block, props);
    }

    @Override
    public Component getName(ItemStack stack) { // VERIFY(26.x): Item#getName(ItemStack)
        return ((LiveBlock) getBlock()).getName();
    }
}
