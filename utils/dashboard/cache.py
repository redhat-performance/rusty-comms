"""
Cache functionality for the Rusty-Comms Dashboard

Provides thread-safe caching with TTL and LRU eviction,
plus decorators for safe and cached computations.
"""

import hashlib
import time
import threading
import logging
import traceback
from functools import wraps

logger = logging.getLogger(__name__)

class EnhancedCache:
    """Thread-safe cache with TTL and size limits for dashboard computations."""
    
    def __init__(self, max_size=100, default_ttl=300):
        self.cache = {}
        self.access_times = {}
        self.creation_times = {}
        self.max_size = max_size
        self.default_ttl = default_ttl
        self._lock = threading.Lock()
    
    def _generate_key(self, *args, **kwargs):
        """Generate a cache key from arguments."""
        key_data = str(args) + str(sorted(kwargs.items()))
        return hashlib.md5(key_data.encode()).hexdigest()
    
    def _is_expired(self, key):
        """Check if a cache entry is expired."""
        if key not in self.creation_times:
            return True
        return time.time() - self.creation_times[key] > self.default_ttl
    
    def _evict_lru(self):
        """Evict least recently used entries."""
        if len(self.cache) >= self.max_size:
            # Remove oldest accessed entries
            sorted_keys = sorted(self.access_times.items(), key=lambda x: x[1])
            keys_to_remove = [k for k, _ in sorted_keys[:len(sorted_keys)//4]]  # Remove 25%
            for key in keys_to_remove:
                self.cache.pop(key, None)
                self.access_times.pop(key, None)
                self.creation_times.pop(key, None)
    
    def get(self, key):
        """Get item from cache."""
        with self._lock:
            if key in self.cache and not self._is_expired(key):
                self.access_times[key] = time.time()
                logger.debug(f"Cache HIT for key: {key[:8]}...")
                return self.cache[key]
            logger.debug(f"Cache MISS for key: {key[:8]}...")
            return None
    
    def put(self, key, value):
        """Put item in cache."""
        with self._lock:
            self._evict_lru()
            self.cache[key] = value
            self.access_times[key] = time.time()
            self.creation_times[key] = time.time()
            logger.debug(f"Cache PUT for key: {key[:8]}...")
    
    def clear(self):
        """Clear all cache entries."""
        with self._lock:
            self.cache.clear()
            self.access_times.clear()
            self.creation_times.clear()
            logger.info("Cache cleared")

# Global cache instance
dashboard_cache = EnhancedCache(max_size=50, default_ttl=600)  # 10 minutes TTL

def cached_computation(ttl=None):
    """Decorator for caching expensive computations with error handling."""
    def decorator(func):
        @wraps(func)
        def wrapper(*args, **kwargs):
            # Generate cache key
            cache_key = dashboard_cache._generate_key(func.__name__, *args, **kwargs)
            
            # Try to get from cache
            cached_result = dashboard_cache.get(cache_key)
            if cached_result is not None:
                return cached_result
            
            # Compute and cache result with error handling
            try:
                start_time = time.time()
                result = func(*args, **kwargs)
                computation_time = time.time() - start_time
                
                logger.info(f"{func.__name__} computed in {computation_time:.2f}s")
                dashboard_cache.put(cache_key, result)
                return result
                
            except Exception as e:
                logger.error(f"Error in {func.__name__}: {str(e)}")
                logger.debug(traceback.format_exc())
                raise
                
        return wrapper
    return decorator

def safe_computation(default_return=None):
    """Decorator for safe computations with comprehensive error handling."""
    def decorator(func):
        @wraps(func)
        def wrapper(*args, **kwargs):
            try:
                return func(*args, **kwargs)
            except Exception as e:
                logger.error(f"Error in {func.__name__}: {str(e)}")
                logger.debug(traceback.format_exc())
                return default_return
        return wrapper
    return decorator
