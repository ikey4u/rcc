#include <iostream>
#include <numeric>
#include <vector>

int main() {
    const std::vector<int> values{1, 2, 3, 4};
    std::cout << "rcc-cxx-ok:" << std::accumulate(values.begin(), values.end(), 0) << '\n';
    return 0;
}
