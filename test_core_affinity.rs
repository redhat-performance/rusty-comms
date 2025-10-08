use core_affinity;

fn main() {
    println!("Testing core_affinity crate...");
    
    // Get available cores
    if let Some(cores) = core_affinity::get_core_ids() {
        println!("Available cores: {:?}", cores);
        println!("Number of cores: {}", cores.len());
        
        // Try to set affinity to core 3
        if let Some(core) = cores.get(3) {
            println!("Attempting to set affinity to core 3 (CoreId: {:?})", core);
            if core_affinity::set_for_current(*core) {
                println!("SUCCESS: Set affinity to core 3");
            } else {
                println!("FAILED: Could not set affinity to core 3");
            }
        } else {
            println!("ERROR: Core 3 not available");
        }
        
        // Try to set affinity to core 2
        if let Some(core) = cores.get(2) {
            println!("Attempting to set affinity to core 2 (CoreId: {:?})", core);
            if core_affinity::set_for_current(*core) {
                println!("SUCCESS: Set affinity to core 2");
            } else {
                println!("FAILED: Could not set affinity to core 2");
            }
        } else {
            println!("ERROR: Core 2 not available");
        }
    } else {
        println!("ERROR: Could not get core IDs");
    }
}


